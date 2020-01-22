//! Hold locks for the process' stdin and stdout.

use once_cell::sync::Lazy;
use parking::{Parker, Unparker};
#[cfg(unix)]
use std::os::unix::io::{AsRawFd, RawFd};
#[cfg(target_os = "wasi")]
use std::os::wasi::io::{AsRawFd, RawFd};
use std::{
    io::{self, stdin, stdout, Stdin, Stdout},
    sync::atomic::{AtomicBool, Ordering::SeqCst},
    thread::{self, JoinHandle},
};
#[cfg(windows)]
use {
    std::os::windows::io::{AsRawHandle, RawHandle},
    unsafe_io::{AsRawHandleOrSocket, RawHandleOrSocket},
};
use system_interface::io::ReadReady;
use unsafe_io::AsUnsafeFile;

// Static handles to `stdin()` and `stdout()` so that we can reference
// them with `StdinLock` and `StdoutLock` with `'static` lifetime
// parameters.
static STDIN: Lazy<Stdin> = Lazy::new(stdin);
static STDOUT: Lazy<Stdout> = Lazy::new(stdout);

// Statically track whether `STDIN` and `STDOUT` are claimed.
static STDIN_CLAIMED: AtomicBool = AtomicBool::new(false);
static STDOUT_CLAIMED: AtomicBool = AtomicBool::new(false);

/// This class acquires a lock on `stdin` and prevents applications from
/// accidentally accessing it through other means.
pub(crate) struct StdinLocker {
    unparker: Unparker,
    handle: Option<JoinHandle<()>>,
}

/// This class acquires a lock on `stdout` and prevents applications from
/// accidentally accessing it through other means.
pub(crate) struct StdoutLocker {
    unparker: Unparker,
    handle: Option<JoinHandle<()>>,
}

impl StdinLocker {
    /// An `InputByteStream` can take the value of the process' stdin, in which
    /// case we want it to have exclusive access to `stdin`. Lock the Rust standard
    /// library's `stdin` to prevent accidental misuse.
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
            let handle = Some(
                thread::Builder::new()
                    .name("ensure exclusive access to stdin".to_owned())
                    .stack_size(64)
                    .spawn(move || {
                        let _lock = STDIN.lock();
                        parker.park()
                    })?,
            );

            Ok(Self { unparker, handle })
        } else {
            Err(io::Error::new(
                io::ErrorKind::Other,
                "attempted dual-ownership of stdin",
            ))
        }
    }
}

impl StdoutLocker {
    /// An `OutputByteStream` can take the value of the process' stdout, in which
    /// case we want it to have exclusive access to `stdout`. Lock the Rust standard
    /// library's `stdout` to prevent accidental misuse.
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
            let handle = Some(
                thread::Builder::new()
                    .name("ensure exclusive access to stdout".to_owned())
                    .stack_size(64)
                    .spawn(move || {
                        let _lock = STDOUT.lock();
                        parker.park()
                    })?,
            );

            Ok(Self { unparker, handle })
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
        self.handle.take().unwrap().join().unwrap();
        STDIN_CLAIMED.store(false, SeqCst);
    }
}

impl Drop for StdoutLocker {
    #[inline]
    fn drop(&mut self) {
        self.unparker.unpark();
        self.handle.take().unwrap().join().unwrap();
        STDOUT_CLAIMED.store(false, SeqCst);
    }
}

#[cfg(not(windows))]
impl AsRawFd for StdinLocker {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        STDIN.as_raw_fd()
    }
}

#[cfg(not(windows))]
impl AsRawFd for StdoutLocker {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        STDOUT.as_raw_fd()
    }
}

#[cfg(windows)]
impl AsRawHandle for StdinLocker {
    #[inline]
    fn as_raw_handle(&self) -> RawHandle {
        STDIN.as_raw_handle()
    }
}

#[cfg(windows)]
impl AsRawHandle for StdoutLocker {
    #[inline]
    fn as_raw_handle(&self) -> RawHandle {
        STDOUT.as_raw_handle()
    }
}

#[cfg(windows)]
impl AsRawHandleOrSocket for StdinLocker {
    #[inline]
    fn as_raw_handle_or_socket(&self) -> RawHandleOrSocket {
        RawHandleOrSocket::from_raw_handle(STDIN.as_raw_handle())
    }
}

#[cfg(windows)]
impl AsRawHandleOrSocket for StdoutLocker {
    #[inline]
    fn as_raw_handle_or_socket(&self) -> RawHandleOrSocket {
        RawHandleOrSocket::from_raw_handle(STDOUT.as_raw_handle())
    }
}

impl ReadReady for StdinLocker {
    #[inline]
    fn num_ready_bytes(&self) -> io::Result<u64> {
        self.as_pipe_reader_view().num_ready_bytes()
    }
}
