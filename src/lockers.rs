//! Hold locks for the process' stdin and stdout.

use io_lifetimes::AsFilelike;
#[cfg(not(windows))]
use io_lifetimes::{AsFd, BorrowedFd};
use os_pipe::PipeReader;
use parking::{Parker, Unparker};
#[cfg(unix)]
use std::os::unix::io::{AsRawFd, RawFd};
#[cfg(target_os = "wasi")]
use std::os::wasi::io::{AsRawFd, RawFd};
use std::{
    io::{self, stdin, stdout},
    sync::atomic::{AtomicBool, Ordering::SeqCst},
    thread::{self, JoinHandle},
};
use system_interface::io::ReadReady;
#[cfg(windows)]
use {
    io_lifetimes::{AsHandle, BorrowedHandle},
    std::os::windows::io::{AsRawHandle, RawHandle},
    unsafe_io::os::windows::{
        AsHandleOrSocket, AsRawHandleOrSocket, BorrowedHandleOrSocket, RawHandleOrSocket,
    },
};

// Statically track whether stdin and stdout are claimed. This allows us to
// issue an error if they're ever claimed multiple times, instead of just
// hanging.
static STDIN_CLAIMED: AtomicBool = AtomicBool::new(false);
static STDOUT_CLAIMED: AtomicBool = AtomicBool::new(false);

// The locker thread just acquires a lock and parks, so it doesn't need much
// memory. Rust adjusts this up to `PTHREAD_STACK_MIN`/etc. as needed.
#[cfg(not(target_os = "freebsd"))]
const LOCKER_STACK_SIZE: usize = 64;

// On FreeBSD, we reportedly need more than the minimum:
// <https://github.com/sunfishcode/io-streams/issues/3#issuecomment-860028594>
#[cfg(target_os = "freebsd")]
const LOCKER_STACK_SIZE: usize = 32 * 1024;

/// This class acquires a lock on `stdin` and prevents applications from
/// accidentally accessing it through other means.
pub(crate) struct StdinLocker {
    #[cfg(not(windows))]
    raw_fd: RawFd,
    #[cfg(windows)]
    raw_handle: RawHandle,
    unparker: Unparker,
    join_handle: Option<JoinHandle<()>>,
}

/// This class acquires a lock on `stdout` and prevents applications from
/// accidentally accessing it through other means.
pub(crate) struct StdoutLocker {
    #[cfg(not(windows))]
    raw_fd: RawFd,
    #[cfg(windows)]
    raw_handle: RawHandle,
    unparker: Unparker,
    join_handle: Option<JoinHandle<()>>,
}

impl StdinLocker {
    /// An `InputByteStream` can take the value of the process' stdin, in which
    /// case we want it to have exclusive access to `stdin`. Lock the Rust
    /// standard library's `stdin` to prevent accidental misuse.
    ///
    /// Fails if a `StdinLocker` instance already exists.
    pub(crate) fn new() -> io::Result<Self> {
        if STDIN_CLAIMED
            .compare_exchange(false, true, SeqCst, SeqCst)
            .is_ok()
        {
            // `StdinLock` is not `Send`. To let `StdinLocker` be send, hold
            // the lock on a parked thread.
            let parker = Parker::new();
            let unparker = parker.unparker();
            let stdin = stdin();
            #[cfg(not(windows))]
            let raw_fd = stdin.as_raw_fd();
            #[cfg(windows)]
            let raw_handle = stdin.as_raw_handle();
            let join_handle = Some(
                thread::Builder::new()
                    .name("ensure exclusive access to stdin".to_owned())
                    .stack_size(LOCKER_STACK_SIZE)
                    .spawn(move || {
                        let _lock = stdin.lock();
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
                "attempted dual-ownership of stdin",
            ))
        }
    }
}

impl StdoutLocker {
    /// An `OutputByteStream` can take the value of the process' stdout, in
    /// which case we want it to have exclusive access to `stdout`. Lock the
    /// Rust standard library's `stdout` to prevent accidental misuse.
    ///
    /// Fails if a `StdoutLocker` instance already exists.
    pub(crate) fn new() -> io::Result<Self> {
        if STDOUT_CLAIMED
            .compare_exchange(false, true, SeqCst, SeqCst)
            .is_ok()
        {
            // `StdoutLock` is not `Send`. To let `StdoutLocker` be send, hold
            // the lock on a parked thread.
            let parker = Parker::new();
            let unparker = parker.unparker();
            let stdout = stdout();
            #[cfg(not(windows))]
            let raw_fd = stdout.as_raw_fd();
            #[cfg(windows)]
            let raw_handle = stdout.as_raw_handle();
            let join_handle = Some(
                thread::Builder::new()
                    .name("ensure exclusive access to stdout".to_owned())
                    .stack_size(LOCKER_STACK_SIZE)
                    .spawn(move || {
                        let _lock = stdout.lock();
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
                "attempted dual-ownership of stdout",
            ))
        }
    }
}

impl Drop for StdinLocker {
    #[inline]
    fn drop(&mut self) {
        self.unparker.unpark();
        self.join_handle.take().unwrap().join().unwrap();
        STDIN_CLAIMED.store(false, SeqCst);
    }
}

impl Drop for StdoutLocker {
    #[inline]
    fn drop(&mut self) {
        self.unparker.unpark();
        self.join_handle.take().unwrap().join().unwrap();
        STDOUT_CLAIMED.store(false, SeqCst);
    }
}

#[cfg(not(windows))]
impl AsRawFd for StdinLocker {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.raw_fd
    }
}

#[cfg(not(windows))]
impl AsRawFd for StdoutLocker {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.raw_fd
    }
}

#[cfg(windows)]
impl AsRawHandle for StdinLocker {
    #[inline]
    fn as_raw_handle(&self) -> RawHandle {
        self.raw_handle
    }
}

#[cfg(windows)]
impl AsRawHandle for StdoutLocker {
    #[inline]
    fn as_raw_handle(&self) -> RawHandle {
        self.raw_handle
    }
}

#[cfg(windows)]
impl AsRawHandleOrSocket for StdinLocker {
    #[inline]
    fn as_raw_handle_or_socket(&self) -> RawHandleOrSocket {
        RawHandleOrSocket::unowned_from_raw_handle(self.as_raw_handle())
    }
}

#[cfg(windows)]
impl AsRawHandleOrSocket for StdoutLocker {
    #[inline]
    fn as_raw_handle_or_socket(&self) -> RawHandleOrSocket {
        RawHandleOrSocket::unowned_from_raw_handle(self.as_raw_handle())
    }
}

#[cfg(not(windows))]
impl AsFd for StdinLocker {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        unsafe { BorrowedFd::borrow_raw_fd(self.raw_fd) }
    }
}

#[cfg(not(windows))]
impl AsFd for StdoutLocker {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        unsafe { BorrowedFd::borrow_raw_fd(self.raw_fd) }
    }
}

#[cfg(windows)]
impl AsHandle for StdinLocker {
    #[inline]
    fn as_handle(&self) -> BorrowedHandle<'_> {
        unsafe { BorrowedHandle::borrow_raw_handle(self.raw_handle) }
    }
}

#[cfg(windows)]
impl AsHandle for StdoutLocker {
    #[inline]
    fn as_handle(&self) -> BorrowedHandle<'_> {
        unsafe { BorrowedHandle::borrow_raw_handle(self.raw_handle) }
    }
}

#[cfg(windows)]
impl AsHandleOrSocket for StdinLocker {
    #[inline]
    fn as_handle_or_socket(&self) -> BorrowedHandleOrSocket<'_> {
        unsafe {
            BorrowedHandleOrSocket::borrow_raw_handle_or_socket(
                RawHandleOrSocket::unowned_from_raw_handle(self.raw_handle),
            )
        }
    }
}

#[cfg(windows)]
impl AsHandleOrSocket for StdoutLocker {
    #[inline]
    fn as_handle_or_socket(&self) -> BorrowedHandleOrSocket<'_> {
        unsafe {
            BorrowedHandleOrSocket::borrow_raw_handle_or_socket(
                RawHandleOrSocket::unowned_from_raw_handle(self.raw_handle),
            )
        }
    }
}

impl ReadReady for StdinLocker {
    #[inline]
    fn num_ready_bytes(&self) -> io::Result<u64> {
        self.as_filelike_view::<PipeReader>().num_ready_bytes()
    }
}
