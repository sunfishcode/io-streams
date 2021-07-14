//! Hold locks for the process' stdin and stdout.

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
use unsafe_io::{AsUnsafeFile, UnsafeFile};
#[cfg(windows)]
use {
    std::os::windows::io::{AsRawHandle, RawHandle},
    unsafe_io::os::windows::{AsRawHandleOrSocket, RawHandleOrSocket},
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
    unsafe_file: UnsafeFile,
    unparker: Unparker,
    join_handle: Option<JoinHandle<()>>,
}

/// This class acquires a lock on `stdout` and prevents applications from
/// accidentally accessing it through other means.
pub(crate) struct StdoutLocker {
    unsafe_file: UnsafeFile,
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
            let unsafe_file = stdin.as_unsafe_file();
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
                unsafe_file,
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
            let unsafe_file = stdout.as_unsafe_file();
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
                unsafe_file,
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
        self.unsafe_file.as_raw_fd()
    }
}

#[cfg(not(windows))]
impl AsRawFd for StdoutLocker {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.unsafe_file.as_raw_fd()
    }
}

#[cfg(windows)]
impl AsRawHandle for StdinLocker {
    #[inline]
    fn as_raw_handle(&self) -> RawHandle {
        self.unsafe_file.as_raw_handle()
    }
}

#[cfg(windows)]
impl AsRawHandle for StdoutLocker {
    #[inline]
    fn as_raw_handle(&self) -> RawHandle {
        self.unsafe_file.as_raw_handle()
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

impl ReadReady for StdinLocker {
    #[inline]
    fn num_ready_bytes(&self) -> io::Result<u64> {
        self.as_pipe_reader_view().num_ready_bytes()
    }
}
