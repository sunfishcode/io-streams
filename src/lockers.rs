//! Hold locks for the process' stdin and stdout.

use io_lifetimes::AsFilelike;
#[cfg(not(windows))]
use io_lifetimes::{AsFd, BorrowedFd};
use os_pipe::PipeReader;
use parking::{Parker, Unparker};
use paste::paste;
use std::io;
#[cfg(unix)]
use std::os::unix::io::{AsRawFd, RawFd};
#[cfg(target_os = "wasi")]
use std::os::wasi::io::{AsRawFd, RawFd};
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;
use std::thread::{self, JoinHandle};
use system_interface::io::ReadReady;
#[cfg(windows)]
use {
    io_extras::os::windows::{
        AsHandleOrSocket, AsRawHandleOrSocket, BorrowedHandleOrSocket, RawHandleOrSocket,
    },
    io_lifetimes::{AsHandle, BorrowedHandle},
    std::os::windows::io::{AsRawHandle, RawHandle},
};

// The locker thread just acquires a lock and parks, so it doesn't need much
// memory. Rust adjusts this up to `PTHREAD_STACK_MIN`/etc. as needed.
#[cfg(not(target_os = "freebsd"))]
const LOCKER_STACK_SIZE: usize = 64;

// On FreeBSD, we reportedly need more than the minimum:
// <https://github.com/sunfishcode/io-streams/issues/3#issuecomment-860028594>
#[cfg(target_os = "freebsd")]
const LOCKER_STACK_SIZE: usize = 32 * 1024;

macro_rules! std_locker {
    ($name: ident) => {
        paste! {
            static [<$name:upper _CLAIMED>]: AtomicBool = AtomicBool::new(false);

            /// This class acquires a lock on `<$name>` and prevents applications from
            /// accidentally accessing it through other means.
            pub(crate) struct [<$name:camel Locker>] {
                #[cfg(not(windows))]
                raw_fd: RawFd,
                #[cfg(windows)]
                raw_handle: RawHandle,
                unparker: Unparker,
                join_handle: Option<JoinHandle<()>>,
            }

            impl [<$name:camel Locker>] {
                /// An `InputByteStream` can take the value of the process' <$name>, in which
                /// case we want it to have exclusive access to `<$name>`. Lock the Rust
                /// standard library's `<$name>` to prevent accidental misuse.
                ///
                /// Fails if a `<$name:camel>Locker` instance already exists.
                pub(crate) fn new() -> io::Result<Self> {
                    if [<$name:upper _CLAIMED>]
                        .compare_exchange(false, true, SeqCst, SeqCst)
                        .is_ok()
                    {
                        // `StdinLock` is not `Send`. To let `StdinLocker` be send, hold
                        // the lock on a parked thread.
                        let parker = Parker::new();
                        let unparker = parker.unparker();
                        let $name = io::$name();
                        #[cfg(not(windows))]
                        let raw_fd = $name.as_raw_fd();
                        #[cfg(windows)]
                        let raw_handle = $name.as_raw_handle();
                        let join_handle = Some(
                            thread::Builder::new()
                                .name("ensure exclusive access to <$name>".to_owned())
                                .stack_size(LOCKER_STACK_SIZE)
                                .spawn(move || {
                                    let _lock = $name.lock();
                                    parker.park()
                                })?,
                        );

                        Ok(Self {
                            #[cfg(not(windows))]
                            raw_fd,
                            #[cfg(windows)]
                            raw_handle,
                            unparker,
                            join_handle,
                        })
                    } else {
                        Err(io::Error::new(
                            io::ErrorKind::Other,
                            "attempted dual-ownership of <$name>",
                        ))
                    }
                }
            }

            impl Drop for [<$name:camel Locker>] {
                #[inline]
                fn drop(&mut self) {
                    self.unparker.unpark();
                    self.join_handle.take().unwrap().join().unwrap();
                    [<$name:upper _CLAIMED>].store(false, SeqCst);
                }
            }

            #[cfg(not(windows))]
            impl AsRawFd for [<$name:camel Locker>] {
                #[inline]
                fn as_raw_fd(&self) -> RawFd {
                    self.raw_fd
                }
            }

            #[cfg(windows)]
            impl AsRawHandle for [<$name:camel Locker>] {
                #[inline]
                fn as_raw_handle(&self) -> RawHandle {
                    self.raw_handle
                }
            }

            #[cfg(windows)]
            impl AsRawHandleOrSocket for [<$name:camel Locker>] {
                #[inline]
                fn as_raw_handle_or_socket(&self) -> RawHandleOrSocket {
                    RawHandleOrSocket::unowned_from_raw_handle(self.as_raw_handle())
                }
            }

            #[cfg(not(windows))]
            impl AsFd for [<$name:camel Locker>] {
                #[inline]
                fn as_fd(&self) -> BorrowedFd<'_> {
                    unsafe { BorrowedFd::borrow_raw(self.raw_fd) }
                }
            }

            #[cfg(windows)]
            impl AsHandle for [<$name:camel Locker>] {
                #[inline]
                fn as_handle(&self) -> BorrowedHandle<'_> {
                    unsafe { BorrowedHandle::borrow_raw(self.raw_handle) }
                }
            }

            #[cfg(windows)]
            impl AsHandleOrSocket for [<$name:camel Locker>] {
                #[inline]
                fn as_handle_or_socket(&self) -> BorrowedHandleOrSocket<'_> {
                    unsafe {
                        BorrowedHandleOrSocket::borrow_raw(RawHandleOrSocket::unowned_from_raw_handle(
                            self.raw_handle,
                        ))
                    }
                }
            }
        }
    };
}

std_locker!(stdin);
std_locker!(stdout);
std_locker!(stderr);

impl ReadReady for StdinLocker {
    #[inline]
    fn num_ready_bytes(&self) -> io::Result<u64> {
        self.as_filelike_view::<PipeReader>().num_ready_bytes()
    }
}
