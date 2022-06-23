use crate::lockers::{StdinLocker, StdoutLocker};
#[cfg(feature = "char-device")]
use char_device::CharDevice;
use duplex::Duplex;
use io_extras::grip::{AsRawGrip, AsRawReadWriteGrip, FromRawGrip, RawGrip};
#[cfg(windows)]
use io_extras::os::windows::{
    AsHandleOrSocket, AsRawHandleOrSocket, AsRawReadWriteHandleOrSocket, AsReadWriteHandleOrSocket,
    BorrowedHandleOrSocket, RawHandleOrSocket,
};
use io_extras::raw::{RawReadable, RawWriteable};
use io_lifetimes::{FromFilelike, FromSocketlike, IntoFilelike, IntoSocketlike};
use std::fmt::{self, Arguments, Debug};
use std::fs::{File, OpenOptions};
use std::io::{self, IoSlice, IoSliceMut, Read, Seek, Write};
use std::net::TcpStream;
#[cfg(unix)]
use std::os::unix::{
    io::{AsRawFd, RawFd},
    net::UnixStream,
};
#[cfg(target_os = "wasi")]
use std::os::wasi::io::{AsRawFd, RawFd};
use system_interface::io::{Peek, ReadReady};
#[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
use {
    duplex::HalfDuplex,
    socketpair::{socketpair_stream, SocketpairStream},
};
#[cfg(not(windows))]
use {
    io_extras::os::rustix::{AsRawReadWriteFd, AsReadWriteFd},
    io_lifetimes::{AsFd, BorrowedFd},
};
#[cfg(not(target_os = "wasi"))]
use {
    // WASI doesn't support pipes yet
    os_pipe::{pipe, PipeReader, PipeWriter},
    std::{
        io::{copy, Cursor},
        process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio},
        thread::{self, JoinHandle},
    },
};

/// An unbuffered and unlocked input byte stream, implementing [`Read`],
/// abstracted over the source of the input.
///
/// It primarily consists of a single file handle, and also contains any
/// resources needed to safely hold the file handle live.
///
/// Since it is unbuffered, and since many input sources have high per-call
/// overhead, it is often beneficial to wrap this in a [`BufReader`].
///
/// [`BufReader`]: std::io::BufReader
pub struct StreamReader {
    handle: RawReadable,
    resources: ReadResources,
}

/// An unbuffered and unlocked output byte stream, implementing [`Write`],
/// abstracted over the destination of the output.
///
/// It primarily consists of a single file handle, and also contains any
/// resources needed to safely hold the file handle live.
///
/// Since it is unbuffered, and since many destinations have high per-call
/// overhead, it is often beneficial to wrap this in a [`BufWriter`] or
/// [`LineWriter`].
///
/// [`BufWriter`]: std::io::BufWriter
/// [`LineWriter`]: std::io::LineWriter
pub struct StreamWriter {
    handle: RawWriteable,
    resources: WriteResources,
}

/// An unbuffered and unlocked interactive combination input and output stream,
/// implementing [`Read`] and [`Write`].
///
/// This may hold two file descriptors, one for reading and one for writing,
/// such as stdin and stdout, or it may hold one file handle for both
/// reading and writing, such as for a TCP socket.
///
/// There is no `file` constructor, even though [`File`] implements both `Read`
/// and `Write`, because normal files are not interactive. However, there is a
/// `char_device` constructor for [character device files].
///
/// [`File`]: std::fs::File
/// [character device files]: https://docs.rs/char-device/latest/char_device/struct.CharDevice.html
pub struct StreamDuplexer {
    read_handle: RawReadable,
    write_handle: RawWriteable,
    resources: DuplexResources,
}

// The Windows [`HANDLE`] type may be transferred across and shared between
// thread boundaries (despite containing a `*mut void`, which in general isn't
// `Send` or `Sync`).
//
// [`HANDLE`]: std::os::windows::raw::HANDLE
#[cfg(windows)]
unsafe impl Send for StreamReader {}
#[cfg(windows)]
unsafe impl Sync for StreamReader {}
#[cfg(windows)]
unsafe impl Send for StreamWriter {}
#[cfg(windows)]
unsafe impl Sync for StreamWriter {}
#[cfg(windows)]
unsafe impl Send for StreamDuplexer {}
#[cfg(windows)]
unsafe impl Sync for StreamDuplexer {}

/// Additional resources that need to be held in order to keep the stream live.
enum ReadResources {
    File(File),
    TcpStream(TcpStream),
    #[cfg(unix)]
    UnixStream(UnixStream),
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    PipeReader(PipeReader),
    Stdin(StdinLocker),
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    PipedThread(Option<(PipeReader, JoinHandle<io::Result<()>>)>),
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    Child(Child),
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    ChildStdout(ChildStdout),
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    ChildStderr(ChildStderr),
}

/// Additional resources that need to be held in order to keep the stream live.
enum WriteResources {
    File(File),
    TcpStream(TcpStream),
    #[cfg(unix)]
    UnixStream(UnixStream),
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    PipeWriter(PipeWriter),
    Stdout(StdoutLocker),
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    PipedThread(Option<(PipeWriter, JoinHandle<io::Result<Box<dyn Write + Send>>>)>),
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    Child(Child),
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    ChildStdin(ChildStdin),
}

/// Additional resources that need to be held in order to keep the stream live.
enum DuplexResources {
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    PipeReaderWriter((PipeReader, PipeWriter)),
    StdinStdout((StdinLocker, StdoutLocker)),
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    Child(Child),
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    ChildStdoutStdin((ChildStdout, ChildStdin)),
    #[cfg(feature = "char-device")]
    CharDevice(CharDevice),
    DevNull(File),
    TcpStream(TcpStream),
    #[cfg(unix)]
    UnixStream(UnixStream),
    #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
    SocketpairStream(SocketpairStream),
    #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
    SocketedThreadFunc(Option<(SocketpairStream, JoinHandle<io::Result<SocketpairStream>>)>),
    #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
    SocketedThread(
        Option<(
            SocketpairStream,
            JoinHandle<io::Result<Box<dyn HalfDuplex + Send>>>,
        )>,
    ),
    #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
    SocketedThreadReadReady(
        Option<(
            SocketpairStream,
            JoinHandle<io::Result<Box<dyn HalfDuplexReadReady + Send>>>,
        )>,
    ),
}

impl StreamReader {
    /// Read from standard input.
    ///
    /// Unlike [`std::io::stdin`], this `stdin` returns a stream which is
    /// unbuffered and unlocked.
    ///
    /// Since it is unbuffered, it is often beneficial to wrap the resulting
    /// `StreamReader` in a [`BufReader`].
    ///
    /// This acquires a [`std::io::StdinLock`] (in a non-recursive way) to
    /// prevent accesses to `std::io::Stdin` while this is live, and fails if a
    /// `StreamReader` or `StreamDuplexer` for standard input already exists.
    ///
    /// [`BufReader`]: std::io::BufReader
    #[inline]
    pub fn stdin() -> io::Result<Self> {
        let stdin_locker = StdinLocker::new()?;

        // Obtain stdin's handle.
        #[cfg(not(windows))]
        let handle = stdin_locker.as_raw_fd();

        // On Windows, stdin may be connected to a UTF-16 console, which
        // `RawHandleOrSocket` can take care of for us.
        #[cfg(windows)]
        let handle = RawHandleOrSocket::stdin();

        Ok(Self::handle(handle, ReadResources::Stdin(stdin_locker)))
    }

    /// Read from an open file, taking ownership of it.
    ///
    /// This method can be passed a [`std::fs::File`] or similar `File` types.
    #[inline]
    #[must_use]
    pub fn file<Filelike: IntoFilelike + Read + Write + Seek>(filelike: Filelike) -> Self {
        // Safety: We don't implement `From`/`Into` to allow the inner `File`
        // to be extracted, so we don't need to worry that we're granting
        // ambient authorities here.
        Self::_file(File::from_into_filelike(filelike))
    }

    #[inline]
    #[must_use]
    fn _file(file: File) -> Self {
        let handle = file.as_raw_grip();
        Self::handle(handle, ReadResources::File(file))
    }

    /// Read from an open TCP stream, taking ownership of it.
    ///
    /// This method can be passed a [`std::net::TcpStream`] or similar
    /// `TcpStream` types.
    #[inline]
    #[must_use]
    pub fn tcp_stream<Socketlike: IntoSocketlike>(socketlike: Socketlike) -> Self {
        Self::_tcp_stream(TcpStream::from_into_socketlike(socketlike))
    }

    #[inline]
    #[must_use]
    fn _tcp_stream(tcp_stream: TcpStream) -> Self {
        let handle = tcp_stream.as_raw_grip();
        // Safety: We don't implement `From`/`Into` to allow the inner
        // `TcpStream` to be extracted, so we don't need to worry that
        // we're granting ambient authorities here.
        Self::handle(handle, ReadResources::TcpStream(tcp_stream))
    }

    /// Read from an open Unix-domain socket, taking ownership of it.
    #[cfg(unix)]
    #[inline]
    #[must_use]
    pub fn unix_stream(unix_stream: UnixStream) -> Self {
        let handle = unix_stream.as_raw_grip();
        Self::handle(handle, ReadResources::UnixStream(unix_stream))
    }

    /// Read from the reading end of an open pipe, taking ownership of it.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    #[inline]
    #[must_use]
    pub fn pipe_reader(pipe_reader: PipeReader) -> Self {
        let handle = pipe_reader.as_raw_grip();
        Self::handle(handle, ReadResources::PipeReader(pipe_reader))
    }

    /// Spawn the given command and read from its standard output.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    pub fn read_from_command(mut command: Command) -> io::Result<Self> {
        command.stdin(Stdio::null());
        command.stdout(Stdio::piped());
        let child = command.spawn()?;
        let handle = child.stdout.as_ref().unwrap().as_raw_grip();
        Ok(Self::handle(handle, ReadResources::Child(child)))
    }

    /// Read from a child process' standard output, taking ownership of it.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    #[inline]
    #[must_use]
    pub fn child_stdout(child_stdout: ChildStdout) -> Self {
        let handle = child_stdout.as_raw_grip();
        Self::handle(handle, ReadResources::ChildStdout(child_stdout))
    }

    /// Read from a child process' standard error, taking ownership of it.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    #[inline]
    #[must_use]
    pub fn child_stderr(child_stderr: ChildStderr) -> Self {
        let handle = child_stderr.as_raw_grip();
        Self::handle(handle, ReadResources::ChildStderr(child_stderr))
    }

    /// Read from a boxed `Read` implementation, taking ownership of it. This
    /// works by creating a new thread to read the data and write it through a
    /// pipe.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    pub fn piped_thread(mut boxed_read: Box<dyn Read + Send>) -> io::Result<Self> {
        let (pipe_reader, mut pipe_writer) = pipe()?;
        let join_handle = thread::Builder::new()
            .name("piped thread for boxed reader".to_owned())
            .spawn(move || copy(&mut *boxed_read, &mut pipe_writer).map(|_size| ()))?;
        let handle = pipe_reader.as_raw_grip();
        Ok(Self::handle(
            handle,
            ReadResources::PipedThread(Some((pipe_reader, join_handle))),
        ))
    }

    /// Read from the null device, which produces no data.
    pub fn null() -> io::Result<Self> {
        #[cfg(not(windows))]
        {
            Ok(Self::file(File::open("/dev/null")?))
        }

        #[cfg(windows)]
        {
            Ok(Self::file(File::open("nul")?))
        }
    }

    /// Read from the given string.
    #[inline]
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    pub fn str<S: AsRef<str>>(s: S) -> io::Result<Self> {
        Self::bytes(s.as_ref().as_bytes())
    }

    /// Read from the given bytes.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    pub fn bytes(bytes: &[u8]) -> io::Result<Self> {
        // If we can write it to a new pipe without blocking, do so.
        #[cfg(not(any(windows, target_os = "redox")))]
        if bytes.len() <= rustix::io::PIPE_BUF {
            let (pipe_reader, mut pipe_writer) = pipe()?;

            pipe_writer.write_all(bytes)?;
            pipe_writer.flush()?;
            drop(pipe_writer);

            let handle = pipe_reader.as_raw_grip();
            return Ok(Self::handle(handle, ReadResources::PipeReader(pipe_reader)));
        }

        // Otherwise, launch a thread.
        Self::piped_thread(Box::new(Cursor::new(bytes.to_vec())))
    }

    #[inline]
    #[must_use]
    fn handle(handle: RawGrip, resources: ReadResources) -> Self {
        Self {
            handle: unsafe { RawReadable::from_raw_grip(handle) },
            resources,
        }
    }

    fn map_err(&mut self, e: io::Error) -> io::Error {
        match &mut self.resources {
            #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
            ReadResources::PipedThread(piped_thread) => {
                let (pipe_reader, join_handle) = piped_thread.take().unwrap();
                drop(pipe_reader);
                join_handle.join().unwrap().unwrap_err()
            }
            _ => e,
        }
    }
}

impl StreamWriter {
    /// Write to standard output.
    ///
    /// Unlike [`std::io::stdout`], this `stdout` returns a stream which is
    /// unbuffered and unlocked.
    ///
    /// Since it is unbuffered, it is often beneficial to wrap the resulting
    /// `StreamWriter` in a [`BufWriter`] or [`LineWriter`].
    ///
    /// This acquires a [`std::io::StdoutLock`] (in a non-recursive way) to
    /// prevent accesses to `std::io::Stdout` while this is live, and fails if
    /// a `StreamWriter` or `StreamDuplexer` for standard output already
    /// exists.
    ///
    /// [`BufWriter`]: std::io::BufWriter
    /// [`LineWriter`]: std::io::LineWriter
    #[inline]
    pub fn stdout() -> io::Result<Self> {
        let stdout_locker = StdoutLocker::new()?;

        // Obtain stdout's handle.
        #[cfg(not(windows))]
        let handle = stdout_locker.as_raw_fd();

        // On Windows, stdout may be connected to a UTF-16 console, which
        // `RawHandleOrSocket` can take care of for us.
        #[cfg(windows)]
        let handle = RawHandleOrSocket::stdout();

        Ok(Self::handle(handle, WriteResources::Stdout(stdout_locker)))
    }

    /// Write to an open file, taking ownership of it.
    ///
    /// This method can be passed a [`std::fs::File`] or similar `File` types.
    #[inline]
    #[must_use]
    pub fn file<Filelike: IntoFilelike + Read + Write + Seek>(filelike: Filelike) -> Self {
        // Safety: We don't implement `From`/`Into` to allow the inner `File`
        // to be extracted, so we don't need to worry that we're granting
        // ambient authorities here.
        Self::_file(File::from_into_filelike(filelike))
    }

    #[inline]
    #[must_use]
    fn _file(file: File) -> Self {
        let handle = file.as_raw_grip();
        Self::handle(handle, WriteResources::File(file))
    }

    /// Write to an open TCP stream, taking ownership of it.
    ///
    /// This method can be passed a [`std::net::TcpStream`] or similar
    /// `TcpStream` types.
    #[inline]
    #[must_use]
    pub fn tcp_stream<Socketlike: IntoSocketlike>(socketlike: Socketlike) -> Self {
        // Safety: We don't implement `From`/`Into` to allow the inner
        // `TcpStream` to be extracted, so we don't need to worry that we're
        // granting ambient authorities here.
        Self::_tcp_stream(TcpStream::from_into_socketlike(socketlike))
    }

    #[inline]
    #[must_use]
    fn _tcp_stream(tcp_stream: TcpStream) -> Self {
        let handle = tcp_stream.as_raw_grip();
        Self::handle(handle, WriteResources::TcpStream(tcp_stream))
    }

    /// Write to an open Unix-domain stream, taking ownership of it.
    #[cfg(unix)]
    #[inline]
    #[must_use]
    pub fn unix_stream(unix_stream: UnixStream) -> Self {
        let handle = unix_stream.as_raw_grip();
        Self::handle(handle, WriteResources::UnixStream(unix_stream))
    }

    /// Write to the writing end of an open pipe, taking ownership of it.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    #[inline]
    #[must_use]
    pub fn pipe_writer(pipe_writer: PipeWriter) -> Self {
        let handle = pipe_writer.as_raw_grip();
        Self::handle(handle, WriteResources::PipeWriter(pipe_writer))
    }

    /// Spawn the given command and write to its standard input. Its standard
    /// output is redirected to `Stdio::null()`.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    pub fn write_to_command(mut command: Command) -> io::Result<Self> {
        command.stdin(Stdio::piped());
        command.stdout(Stdio::null());
        let child = command.spawn()?;
        let handle = child.stdin.as_ref().unwrap().as_raw_grip();
        Ok(Self::handle(handle, WriteResources::Child(child)))
    }

    /// Write to the given child standard input, taking ownership of it.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    #[inline]
    #[must_use]
    pub fn child_stdin(child_stdin: ChildStdin) -> Self {
        let handle = child_stdin.as_raw_grip();
        Self::handle(handle, WriteResources::ChildStdin(child_stdin))
    }

    /// Write to a boxed `Write` implementation, taking ownership of it. This
    /// works by creating a new thread to read the data through a pipe and
    /// write it.
    ///
    /// Writes to the pipe aren't synchronous with writes to the boxed `Write`
    /// implementation. To ensure data is flushed all the way through the
    /// thread and into the boxed `Write` implementation, call [`flush`]`()`,
    /// which synchronizes with the thread to ensure that is has completed
    /// writing all pending output.
    ///
    /// [`flush`]: https://doc.rust-lang.org/std/io/trait.Write.html#tymethod.flush
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    pub fn piped_thread(mut boxed_write: Box<dyn Write + Send>) -> io::Result<Self> {
        let (mut pipe_reader, pipe_writer) = pipe()?;
        let join_handle = thread::Builder::new()
            .name("piped thread for boxed writer".to_owned())
            .spawn(move || {
                copy(&mut pipe_reader, &mut *boxed_write)?;
                boxed_write.flush()?;
                Ok(boxed_write)
            })?;
        let handle = pipe_writer.as_raw_grip();
        Ok(Self::handle(
            handle,
            WriteResources::PipedThread(Some((pipe_writer, join_handle))),
        ))
    }

    /// Write to the null device, which ignores all data.
    pub fn null() -> io::Result<Self> {
        #[cfg(not(windows))]
        {
            Ok(Self::file(File::create("/dev/null")?))
        }

        #[cfg(windows)]
        {
            Ok(Self::file(File::create("nul")?))
        }
    }

    #[inline]
    fn handle(handle: RawGrip, resources: WriteResources) -> Self {
        Self {
            handle: unsafe { RawWriteable::from_raw_grip(handle) },
            resources,
        }
    }

    fn map_err(&mut self, e: io::Error) -> io::Error {
        match &mut self.resources {
            #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
            WriteResources::PipedThread(piped_thread) => {
                let (pipe_writer, join_handle) = piped_thread.take().unwrap();
                drop(pipe_writer);
                join_handle.join().unwrap().map(|_| ()).unwrap_err()
            }
            _ => e,
        }
    }
}

impl StreamDuplexer {
    /// Duplex with stdin and stdout, taking ownership of them.
    ///
    /// Unlike [`std::io::stdin`] and [`std::io::stdout`], this `stdin_stdout`
    /// returns a stream which is unbuffered and unlocked.
    ///
    /// This acquires a [`std::io::StdinLock`] and a [`std::io::StdoutLock`]
    /// (in non-recursive ways) to prevent accesses to [`std::io::Stdin`] and
    /// [`std::io::Stdout`] while this is live, and fails if a `StreamReader`
    /// for standard input, a `StreamWriter` for standard output, or a
    /// `StreamDuplexer` for standard input and standard output already exist.
    #[inline]
    pub fn stdin_stdout() -> io::Result<Self> {
        let stdin_locker = StdinLocker::new()?;
        let stdout_locker = StdoutLocker::new()?;

        // Obtain stdin's and stdout's handles.
        #[cfg(not(windows))]
        let (read, write) = (stdin_locker.as_raw_grip(), stdout_locker.as_raw_grip());

        // On Windows, stdin and stdout may be connected to a UTF-16 console,
        // which `RawHandleOrSocket` can take care of for us.
        #[cfg(windows)]
        let (read, write) = (RawHandleOrSocket::stdin(), RawHandleOrSocket::stdout());

        Ok(Self::two_handles(
            read,
            write,
            DuplexResources::StdinStdout((stdin_locker, stdout_locker)),
        ))
    }

    /// Duplex with an open character device, taking ownership of it.
    #[cfg(feature = "char-device")]
    #[cfg_attr(doc_cfg, doc(cfg(feature = "char-device")))]
    #[inline]
    #[must_use]
    pub fn char_device(char_device: CharDevice) -> Self {
        let handle = char_device.as_raw_grip();
        Self::handle(handle, DuplexResources::CharDevice(char_device))
    }

    /// Duplex with an open TCP stream, taking ownership of it.
    ///
    /// This method can be passed a [`std::net::TcpStream`] or similar
    /// `TcpStream` types.
    #[inline]
    #[must_use]
    pub fn tcp_stream<Socketlike: IntoSocketlike>(socketlike: Socketlike) -> Self {
        Self::_tcp_stream(TcpStream::from_into_socketlike(socketlike))
    }

    #[inline]
    #[must_use]
    fn _tcp_stream(tcp_stream: TcpStream) -> Self {
        let handle = tcp_stream.as_raw_grip();
        // Safety: We don't implement `From`/`Into` to allow the inner
        // `TcpStream` to be extracted, so we don't need to worry that
        // we're granting ambient authorities here.
        Self::handle(handle, DuplexResources::TcpStream(tcp_stream))
    }

    /// Duplex with an open Unix-domain stream, taking ownership of it.
    #[cfg(unix)]
    #[must_use]
    pub fn unix_stream(unix_stream: UnixStream) -> Self {
        let handle = unix_stream.as_raw_grip();
        Self::handle(handle, DuplexResources::UnixStream(unix_stream))
    }

    /// Duplex with a pair of pipe streams, taking ownership of them.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    #[inline]
    #[must_use]
    pub fn pipe_reader_writer(pipe_reader: PipeReader, pipe_writer: PipeWriter) -> Self {
        let read = pipe_reader.as_raw_grip();
        let write = pipe_writer.as_raw_grip();
        Self::two_handles(
            read,
            write,
            DuplexResources::PipeReaderWriter((pipe_reader, pipe_writer)),
        )
    }

    /// Duplex with one end of a socketpair stream, taking ownership of it.
    #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
    #[cfg_attr(
        doc_cfg,
        doc(cfg(all(not(target_os = "wasi"), feature = "socketpair")))
    )]
    #[must_use]
    pub fn socketpair_stream(stream: SocketpairStream) -> Self {
        let handle = stream.as_raw_grip();
        Self::handle(handle, DuplexResources::SocketpairStream(stream))
    }

    /// Spawn the given command and duplex with its standard input and output.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    pub fn duplex_with_command(mut command: Command) -> io::Result<Self> {
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        let child = command.spawn()?;
        let read = child.stdout.as_ref().unwrap().as_raw_grip();
        let write = child.stdin.as_ref().unwrap().as_raw_grip();
        Ok(Self::two_handles(
            read,
            write,
            DuplexResources::Child(child),
        ))
    }

    /// Duplex with a child process' stdout and stdin, taking ownership of
    /// them.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    #[inline]
    #[must_use]
    pub fn child_stdout_stdin(child_stdout: ChildStdout, child_stdin: ChildStdin) -> Self {
        let read = child_stdout.as_raw_grip();
        let write = child_stdin.as_raw_grip();
        Self::two_handles(
            read,
            write,
            DuplexResources::ChildStdoutStdin((child_stdout, child_stdin)),
        )
    }

    /// Duplex with a duplexer from on another thread through a socketpair.
    ///
    /// A socketpair is created, new thread is created, `boxed_duplex` is
    /// read from and written to over the socketpair.
    ///
    /// Writes to the pipe aren't synchronous with writes to the boxed `Write`
    /// implementation. To ensure data is flushed all the way through the
    /// thread and into the boxed `Write` implementation, call `flush()`, which
    /// synchronizes with the thread to ensure that is has completed writing
    /// all pending output.
    #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
    #[cfg_attr(
        doc_cfg,
        doc(cfg(all(not(target_os = "wasi"), feature = "socketpair")))
    )]
    pub fn socketed_thread_read_first(
        mut boxed_duplex: Box<dyn HalfDuplex + Send>,
    ) -> io::Result<Self> {
        let (a, b) = socketpair_stream()?;
        let join_handle = thread::Builder::new()
            .name("socketed thread for boxed duplexer".to_owned())
            .spawn(move || {
                read_first(a, &mut *boxed_duplex)?;
                Ok(boxed_duplex)
            })?;
        let handle = b.as_raw_grip();
        Ok(Self::handle(
            handle,
            DuplexResources::SocketedThread(Some((b, join_handle))),
        ))
    }

    /// Duplex with a duplexer from on another thread through a socketpair.
    ///
    /// A socketpair is created, new thread is created, `boxed_duplex` is
    /// written to and read from over the socketpair.
    ///
    /// Writes to the pipe aren't synchronous with writes to the boxed `Write`
    /// implementation. To ensure data is flushed all the way through the
    /// thread and into the boxed `Write` implementation, call `flush()`, which
    /// synchronizes with the thread to ensure that is has completed writing
    /// all pending output.
    #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
    #[cfg_attr(
        doc_cfg,
        doc(cfg(all(not(target_os = "wasi"), feature = "socketpair")))
    )]
    pub fn socketed_thread_write_first(
        mut boxed_duplex: Box<dyn HalfDuplex + Send>,
    ) -> io::Result<Self> {
        let (a, b) = socketpair_stream()?;
        let join_handle = thread::Builder::new()
            .name("socketed thread for boxed duplexer".to_owned())
            .spawn(move || {
                write_first(a, &mut *boxed_duplex)?;
                Ok(boxed_duplex)
            })?;
        let handle = b.as_raw_grip();
        Ok(Self::handle(
            handle,
            DuplexResources::SocketedThread(Some((b, join_handle))),
        ))
    }

    /// Duplex with a duplexer from on another thread through a socketpair.
    ///
    /// A socketpair is created, new thread is created, `boxed_duplex` is
    /// written to and/or read from over the socketpair.
    /// `ReadReady::num_ready_bytes` is used to determine whether to read from
    /// or write to `boxed_duplex` first. This may be inefficient, so if you
    /// know which direction should go first, use `socketed_thread_read_first`
    /// or `socketed_thread_write_first` instead.
    ///
    /// Writes to the pipe aren't synchronous with writes to the boxed `Write`
    /// implementation. To ensure data is flushed all the way through the
    /// thread and into the boxed `Write` implementation, call `flush()`, which
    /// synchronizes with the thread to ensure that is has completed writing
    /// all pending output.
    #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
    #[cfg_attr(
        doc_cfg,
        doc(cfg(all(not(target_os = "wasi"), feature = "socketpair")))
    )]
    pub fn socketed_thread(
        mut boxed_duplex: Box<dyn HalfDuplexReadReady + Send>,
    ) -> io::Result<Self> {
        let (a, b) = socketpair_stream()?;
        let join_handle = thread::Builder::new()
            .name("socketed thread for boxed duplexer".to_owned())
            .spawn(move || {
                loop {
                    if a.num_ready_bytes()? != 0 {
                        write_first(a, &mut *boxed_duplex)?;
                        break;
                    }
                    if boxed_duplex.num_ready_bytes()? != 0 {
                        read_first(a, &mut *boxed_duplex)?;
                        break;
                    }

                    // TODO: Implement a nicer way to wait for either of
                    // the streams to be ready to read.
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
                Ok(boxed_duplex)
            })?;
        let handle = b.as_raw_grip();
        Ok(Self::handle(
            handle,
            DuplexResources::SocketedThreadReadReady(Some((b, join_handle))),
        ))
    }

    /// Duplex with a function running on another thread through a socketpair.
    ///
    /// A socketpair is created, new thread is created, `func` is called in the
    /// new thread and passed one of the ends of the socketstream.
    ///
    /// Writes to the pipe aren't synchronous with writes to the boxed `Write`
    /// implementation. To ensure data is flushed all the way through the
    /// thread and into the boxed `Write` implementation, call `flush()`, which
    /// synchronizes with the thread to ensure that is has completed writing
    /// all pending output.
    #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
    #[cfg_attr(
        doc_cfg,
        doc(cfg(all(not(target_os = "wasi"), feature = "socketpair")))
    )]
    pub fn socketed_thread_func(
        func: Box<dyn Send + FnOnce(SocketpairStream) -> io::Result<SocketpairStream>>,
    ) -> io::Result<Self> {
        let (a, b) = socketpair_stream()?;
        let join_handle = thread::Builder::new()
            .name("socketed thread for boxed duplexer".to_owned())
            .spawn(move || func(a))?;
        let handle = b.as_raw_grip();
        Ok(Self::handle(
            handle,
            DuplexResources::SocketedThreadFunc(Some((b, join_handle))),
        ))
    }

    /// Read and write with the null device, which ignores all data, and
    /// produces no data.
    pub fn null() -> io::Result<Self> {
        #[cfg(not(windows))]
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")?;

        #[cfg(windows)]
        let file = OpenOptions::new().read(true).write(true).open("nul")?;

        let handle = file.as_raw_grip();
        Ok(Self::handle(handle, DuplexResources::DevNull(file)))
    }

    #[inline]
    #[must_use]
    fn handle(handle: RawGrip, resources: DuplexResources) -> Self {
        Self {
            read_handle: unsafe { RawReadable::from_raw_grip(handle) },
            write_handle: unsafe { RawWriteable::from_raw_grip(handle) },
            resources,
        }
    }

    #[inline]
    #[must_use]
    fn two_handles(read: RawGrip, write: RawGrip, resources: DuplexResources) -> Self {
        Self {
            read_handle: unsafe { RawReadable::from_raw_grip(read) },
            write_handle: unsafe { RawWriteable::from_raw_grip(write) },
            resources,
        }
    }

    fn map_err(&mut self, e: io::Error) -> io::Error {
        match &mut self.resources {
            _ => e,
        }
    }
}

impl Read for StreamReader {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.handle.read(buf) {
            Ok(size) => Ok(size),
            Err(e) => Err(self.map_err(e)),
        }
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [IoSliceMut]) -> io::Result<usize> {
        match self.handle.read_vectored(bufs) {
            Ok(size) => Ok(size),
            Err(e) => Err(self.map_err(e)),
        }
    }

    #[cfg(can_vector)]
    #[inline]
    fn is_read_vectored(&self) -> bool {
        self.handle.is_read_vectored()
    }

    #[inline]
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        match self.handle.read_to_end(buf) {
            Ok(size) => Ok(size),
            Err(e) => Err(self.map_err(e)),
        }
    }

    #[inline]
    fn read_to_string(&mut self, buf: &mut String) -> io::Result<usize> {
        match self.handle.read_to_string(buf) {
            Ok(size) => Ok(size),
            Err(e) => Err(self.map_err(e)),
        }
    }

    #[inline]
    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        match self.handle.read_exact(buf) {
            Ok(()) => Ok(()),
            Err(e) => Err(self.map_err(e)),
        }
    }
}

impl Peek for StreamReader {
    fn peek(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match &mut self.resources {
            ReadResources::File(file) => Peek::peek(file, buf),
            ReadResources::TcpStream(tcp_stream) => Peek::peek(tcp_stream, buf),
            #[cfg(unix)]
            ReadResources::UnixStream(unix_stream) => Peek::peek(unix_stream, buf),
            _ => Ok(0),
        }
    }
}

impl ReadReady for StreamReader {
    fn num_ready_bytes(&self) -> io::Result<u64> {
        match &self.resources {
            ReadResources::File(file) => ReadReady::num_ready_bytes(file),
            ReadResources::TcpStream(tcp_stream) => ReadReady::num_ready_bytes(tcp_stream),
            #[cfg(unix)]
            ReadResources::UnixStream(unix_stream) => ReadReady::num_ready_bytes(unix_stream),
            ReadResources::PipeReader(pipe_reader) => ReadReady::num_ready_bytes(pipe_reader),
            ReadResources::Stdin(stdin) => ReadReady::num_ready_bytes(stdin),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::PipedThread(piped_thread) => {
                ReadReady::num_ready_bytes(&piped_thread.as_ref().unwrap().0)
            }
            #[cfg(not(target_os = "wasi"))]
            ReadResources::Child(child) => {
                ReadReady::num_ready_bytes(child.stdout.as_ref().unwrap())
            }
            #[cfg(not(target_os = "wasi"))]
            ReadResources::ChildStdout(child_stdout) => ReadReady::num_ready_bytes(child_stdout),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::ChildStderr(child_stderr) => ReadReady::num_ready_bytes(child_stderr),
        }
    }
}

impl Write for StreamWriter {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.handle.write(buf) {
            Ok(size) => Ok(size),
            Err(e) => Err(self.map_err(e)),
        }
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        match self.handle.flush() {
            Ok(()) => {
                // There's no way to send a flush event through a pipe, so for
                // now, force a flush by closing the pipe, waiting for the
                // thread to exit, recover the boxed writer, and then wrap it
                // in a whole new piped thread.
                #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
                if let WriteResources::PipedThread(piped_thread) = &mut self.resources {
                    let (mut pipe_writer, join_handle) = piped_thread.take().unwrap();
                    pipe_writer.flush()?;
                    drop(pipe_writer);
                    let boxed_write = join_handle.join().unwrap().unwrap();
                    *self = Self::piped_thread(boxed_write)?;
                }
                Ok(())
            }
            Err(e) => Err(self.map_err(e)),
        }
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[IoSlice]) -> io::Result<usize> {
        match self.handle.write_vectored(bufs) {
            Ok(size) => Ok(size),
            Err(e) => Err(self.map_err(e)),
        }
    }

    #[cfg(can_vector)]
    #[inline]
    fn is_write_vectored(&self) -> bool {
        self.handle.is_write_vectored()
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match self.handle.write_all(buf) {
            Ok(()) => Ok(()),
            Err(e) => Err(self.map_err(e)),
        }
    }

    #[cfg(write_all_vectored)]
    #[inline]
    fn write_all_vectored(&mut self, bufs: &mut [IoSlice]) -> io::Result<()> {
        match self.handle.write_all_vectored(bufs) {
            Ok(()) => Ok(()),
            Err(e) => Err(self.map_err(e)),
        }
    }

    #[inline]
    fn write_fmt(&mut self, fmt: Arguments) -> io::Result<()> {
        match self.handle.write_fmt(fmt) {
            Ok(()) => Ok(()),
            Err(e) => Err(self.map_err(e)),
        }
    }
}

impl Read for StreamDuplexer {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.read_handle.read(buf) {
            Ok(size) => Ok(size),
            Err(e) => Err(self.map_err(e)),
        }
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [IoSliceMut]) -> io::Result<usize> {
        match self.read_handle.read_vectored(bufs) {
            Ok(size) => Ok(size),
            Err(e) => Err(self.map_err(e)),
        }
    }

    #[cfg(can_vector)]
    #[inline]
    fn is_read_vectored(&self) -> bool {
        self.read_handle.is_read_vectored()
    }

    #[inline]
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        match self.read_handle.read_to_end(buf) {
            Ok(size) => Ok(size),
            Err(e) => Err(self.map_err(e)),
        }
    }

    #[inline]
    fn read_to_string(&mut self, buf: &mut String) -> io::Result<usize> {
        match self.read_handle.read_to_string(buf) {
            Ok(size) => Ok(size),
            Err(e) => Err(self.map_err(e)),
        }
    }

    #[inline]
    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        match self.read_handle.read_exact(buf) {
            Ok(()) => Ok(()),
            Err(e) => Err(self.map_err(e)),
        }
    }
}

impl Peek for StreamDuplexer {
    fn peek(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match &mut self.resources {
            DuplexResources::TcpStream(tcp_stream) => Peek::peek(tcp_stream, buf),
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => Peek::peek(unix_stream, buf),
            #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
            DuplexResources::SocketpairStream(socketpair) => Peek::peek(socketpair, buf),
            #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
            DuplexResources::SocketedThreadFunc(socketed_thread) => {
                Peek::peek(&mut socketed_thread.as_mut().unwrap().0, buf)
            }
            #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
            DuplexResources::SocketedThreadReadReady(socketed_thread) => {
                Peek::peek(&mut socketed_thread.as_mut().unwrap().0, buf)
            }
            _ => Ok(0),
        }
    }
}

impl ReadReady for StreamDuplexer {
    fn num_ready_bytes(&self) -> io::Result<u64> {
        match &self.resources {
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::PipeReaderWriter((pipe_reader, _)) => {
                ReadReady::num_ready_bytes(pipe_reader)
            }
            DuplexResources::StdinStdout((stdin, _)) => ReadReady::num_ready_bytes(stdin),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(child) => {
                ReadReady::num_ready_bytes(child.stdout.as_ref().unwrap())
            }
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin((child_stdout, _)) => {
                ReadReady::num_ready_bytes(child_stdout)
            }
            #[cfg(feature = "char-device")]
            DuplexResources::CharDevice(char_device) => ReadReady::num_ready_bytes(char_device),
            DuplexResources::DevNull(file) => ReadReady::num_ready_bytes(file),
            DuplexResources::TcpStream(tcp_stream) => ReadReady::num_ready_bytes(tcp_stream),
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => ReadReady::num_ready_bytes(unix_stream),
            #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
            DuplexResources::SocketpairStream(socketpair_stream) => {
                ReadReady::num_ready_bytes(socketpair_stream)
            }
            #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
            DuplexResources::SocketedThreadFunc(socketed_thread) => {
                ReadReady::num_ready_bytes(&socketed_thread.as_ref().unwrap().0)
            }
            #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
            DuplexResources::SocketedThread(socketed_thread) => {
                ReadReady::num_ready_bytes(&socketed_thread.as_ref().unwrap().0)
            }
            #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
            DuplexResources::SocketedThreadReadReady(socketed_thread) => {
                ReadReady::num_ready_bytes(&socketed_thread.as_ref().unwrap().0)
            }
        }
    }
}

impl Write for StreamDuplexer {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.write_handle.write(buf) {
            Ok(size) => Ok(size),
            Err(e) => Err(self.map_err(e)),
        }
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        match self.write_handle.flush() {
            Ok(()) => Ok(()),
            Err(e) => Err(self.map_err(e)),
        }
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[IoSlice]) -> io::Result<usize> {
        match self.write_handle.write_vectored(bufs) {
            Ok(size) => Ok(size),
            Err(e) => Err(self.map_err(e)),
        }
    }

    #[cfg(can_vector)]
    #[inline]
    fn is_write_vectored(&self) -> bool {
        self.write_handle.is_write_vectored()
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match self.write_handle.write_all(buf) {
            Ok(()) => Ok(()),
            Err(e) => Err(self.map_err(e)),
        }
    }

    #[cfg(write_all_vectored)]
    #[inline]
    fn write_all_vectored(&mut self, bufs: &mut [IoSlice]) -> io::Result<()> {
        match self.write_handle.write_all_vectored(bufs) {
            Ok(()) => Ok(()),
            Err(e) => Err(self.map_err(e)),
        }
    }

    #[inline]
    fn write_fmt(&mut self, fmt: Arguments) -> io::Result<()> {
        match self.write_handle.write_fmt(fmt) {
            Ok(()) => Ok(()),
            Err(e) => Err(self.map_err(e)),
        }
    }
}

impl Duplex for StreamDuplexer {}

#[cfg(not(windows))]
impl AsRawFd for StreamReader {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.handle.as_raw_fd()
    }
}

#[cfg(not(windows))]
impl AsRawFd for StreamWriter {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.handle.as_raw_fd()
    }
}

#[cfg(not(windows))]
impl AsRawReadWriteFd for StreamDuplexer {
    #[inline]
    fn as_raw_read_fd(&self) -> RawFd {
        self.read_handle.as_raw_fd()
    }

    #[inline]
    fn as_raw_write_fd(&self) -> RawFd {
        self.write_handle.as_raw_fd()
    }
}

#[cfg(windows)]
impl AsRawHandleOrSocket for StreamReader {
    #[inline]
    fn as_raw_handle_or_socket(&self) -> RawHandleOrSocket {
        self.handle.as_raw_handle_or_socket()
    }
}

#[cfg(windows)]
impl AsRawHandleOrSocket for StreamWriter {
    #[inline]
    fn as_raw_handle_or_socket(&self) -> RawHandleOrSocket {
        self.handle.as_raw_handle_or_socket()
    }
}

#[cfg(windows)]
impl AsRawReadWriteHandleOrSocket for StreamDuplexer {
    #[inline]
    fn as_raw_read_handle_or_socket(&self) -> RawHandleOrSocket {
        self.read_handle.as_raw_handle_or_socket()
    }

    #[inline]
    fn as_raw_write_handle_or_socket(&self) -> RawHandleOrSocket {
        self.write_handle.as_raw_handle_or_socket()
    }
}

#[cfg(not(windows))]
impl AsFd for StreamReader {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        unsafe { BorrowedFd::borrow_raw(self.handle.as_raw_fd()) }
    }
}

#[cfg(not(windows))]
impl AsFd for StreamWriter {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        unsafe { BorrowedFd::borrow_raw(self.handle.as_raw_fd()) }
    }
}

#[cfg(not(windows))]
impl AsReadWriteFd for StreamDuplexer {
    #[inline]
    fn as_read_fd(&self) -> BorrowedFd<'_> {
        unsafe { BorrowedFd::borrow_raw(self.read_handle.as_raw_fd()) }
    }

    #[inline]
    fn as_write_fd(&self) -> BorrowedFd<'_> {
        unsafe { BorrowedFd::borrow_raw(self.write_handle.as_raw_fd()) }
    }
}

#[cfg(windows)]
impl AsHandleOrSocket for StreamReader {
    #[inline]
    fn as_handle_or_socket(&self) -> BorrowedHandleOrSocket<'_> {
        unsafe { BorrowedHandleOrSocket::borrow_raw(self.handle.as_raw_handle_or_socket()) }
    }
}

#[cfg(windows)]
impl AsHandleOrSocket for StreamWriter {
    #[inline]
    fn as_handle_or_socket(&self) -> BorrowedHandleOrSocket<'_> {
        unsafe { BorrowedHandleOrSocket::borrow_raw(self.handle.as_raw_handle_or_socket()) }
    }
}

#[cfg(windows)]
impl AsReadWriteHandleOrSocket for StreamDuplexer {
    #[inline]
    fn as_read_handle_or_socket(&self) -> BorrowedHandleOrSocket<'_> {
        unsafe { BorrowedHandleOrSocket::borrow_raw(self.read_handle.as_raw_handle_or_socket()) }
    }

    #[inline]
    fn as_write_handle_or_socket(&self) -> BorrowedHandleOrSocket<'_> {
        unsafe { BorrowedHandleOrSocket::borrow_raw(self.write_handle.as_raw_handle_or_socket()) }
    }
}

impl Drop for ReadResources {
    fn drop(&mut self) {
        match self {
            #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
            Self::PipedThread(piped_thread) => {
                let (pipe_reader, join_handle) = piped_thread.take().unwrap();
                drop(pipe_reader);
                // If the thread was still writing, we may have just caused
                // it to fail with `BrokenPipe`; ignore such errors because
                // we're dropping the stream.
                match join_handle.join().unwrap() {
                    Ok(()) => (),
                    Err(e) if e.kind() == io::ErrorKind::BrokenPipe => (),
                    Err(e) => Err(e).unwrap(),
                }
            }
            _ => {}
        }
    }
}

impl Drop for WriteResources {
    fn drop(&mut self) {
        match self {
            #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
            Self::PipedThread(piped_thread) => {
                if let Some((pipe_writer, join_handle)) = piped_thread.take() {
                    drop(pipe_writer);
                    join_handle.join().unwrap().unwrap();
                }
            }
            _ => {}
        }
    }
}

impl Drop for DuplexResources {
    fn drop(&mut self) {
        match self {
            #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
            Self::SocketedThreadFunc(socketed_thread) => {
                if let Some((socketpair, join_handle)) = socketed_thread.take() {
                    drop(socketpair);
                    join_handle.join().unwrap().unwrap();
                }
            }
            _ => {}
        }
    }
}

impl Debug for StreamReader {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Just print the fd number; don't try to print the path or any
        // information about it, because this information is otherwise
        // unavailable to safe Rust code.
        f.debug_struct("StreamReader")
            .field("raw_grip", &self.as_raw_grip())
            .finish()
    }
}

impl Debug for StreamWriter {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Just print the fd number; don't try to print the path or any
        // information about it, because this information is otherwise
        // unavailable to safe Rust code.
        f.debug_struct("StreamWriter")
            .field("raw_grip", &self.as_raw_grip())
            .finish()
    }
}

impl Debug for StreamDuplexer {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Just print the fd numbers; don't try to print the path or any
        // information about it, because this information is otherwise
        // unavailable to safe Rust code.
        f.debug_struct("StreamDuplexer")
            .field("unsafe_readable", &self.as_raw_read_grip())
            .field("unsafe_writeable", &self.as_raw_write_grip())
            .finish()
    }
}

/// A trait that combines [`HalfDuplex`] and [`ReadReady`]. Implemented via
/// blanket implementation.
#[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
#[cfg_attr(
    doc_cfg,
    doc(cfg(all(not(target_os = "wasi"), feature = "socketpair")))
)]
pub trait HalfDuplexReadReady: HalfDuplex + ReadReady {}

#[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
impl<T: HalfDuplex + ReadReady> HalfDuplexReadReady for T {}

#[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
fn read_first<T: HalfDuplex + ?Sized>(
    mut a: SocketpairStream,
    boxed_duplex: &mut T,
) -> io::Result<()> {
    'thread: loop {
        let mut buf = vec![0_u8; crate::buffered::DEFAULT_BUF_SIZE];

        loop {
            let n = match boxed_duplex.read(&mut buf) {
                Ok(0) => break 'thread,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => 0,
                Err(e) => return Err(e),
            };
            a.write_all(&buf[..n])?;
            if n < buf.len() {
                break;
            }
        }

        loop {
            let n = match a.read(&mut buf) {
                Ok(0) => break 'thread,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => 0,
                Err(e) => return Err(e),
            };
            boxed_duplex.write_all(&buf[..n])?;
            if n < buf.len() {
                break;
            }
        }
    }
    Ok(())
}

#[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
fn write_first<T: HalfDuplex + ?Sized>(
    mut a: SocketpairStream,
    boxed_duplex: &mut T,
) -> io::Result<()> {
    'thread: loop {
        let mut buf = [0_u8; crate::buffered::DEFAULT_BUF_SIZE];

        loop {
            let n = match a.read(&mut buf) {
                Ok(0) => break 'thread,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => 0,
                Err(e) => return Err(e),
            };
            boxed_duplex.write_all(&buf[..n])?;
            if n < buf.len() {
                break;
            }
        }

        loop {
            let n = match boxed_duplex.read(&mut buf) {
                Ok(0) => break 'thread,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => 0,
                Err(e) => return Err(e),
            };
            a.write_all(&buf[..n])?;
            if n < buf.len() {
                break;
            }
        }
    }
    Ok(())
}
