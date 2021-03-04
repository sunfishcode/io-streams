use crate::lockers::{StdinLocker, StdoutLocker};
#[cfg(unix)]
use async_std::os::unix::{
    io::{AsRawFd, RawFd},
    net::UnixStream,
};
#[cfg(target_os = "wasi")]
use async_std::os::wasi::io::{AsRawFd, RawFd};
use async_std::{
    fs::File,
    io::{self, IoSlice, IoSliceMut, Read, Seek, Write},
    net::TcpStream,
};
#[cfg(feature = "char-device")]
use char_device::AsyncCharDevice;
use duplex::Duplex;
use std::{
    fmt::{self, Debug},
    pin::Pin,
    task::{Context, Poll},
};
use system_interface::io::ReadReady;
#[cfg(not(windows))]
use unsafe_io::os::posish::AsRawReadWriteFd;
#[cfg(windows)]
use unsafe_io::os::windows::{
    AsRawHandleOrSocket, AsRawReadWriteHandleOrSocket, RawHandleOrSocket,
};
use unsafe_io::{
    AsUnsafeHandle, AsUnsafeReadWriteHandle, FromUnsafeFile, FromUnsafeSocket, IntoUnsafeFile,
    IntoUnsafeSocket, OwnsRaw,
};
#[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
use {duplex::HalfDuplex, socketpair::AsyncSocketpairStream};
#[cfg(not(target_os = "wasi"))]
use {
    os_pipe::{PipeReader, PipeWriter},
    std::{
        process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
        thread::JoinHandle,
    },
};

/// An unbuffered and unlocked input byte stream, abstracted over the source of
/// the input.
///
/// Since it is unbuffered, and since many input sources have high per-call
/// overhead, it is often beneficial to wrap this in a [`BufReader`].
///
/// TODO: "Unbuffered" here isn't entirely accurate, given how async-std deals
/// with the underlying OS APIs being effectively synchronous. Figure out what
/// to say here.
///
/// [`BufReader`]: async_std::io::BufReader
pub struct AsyncStreamReader {
    resources: ReadResources,
}

/// An unbuffered and unlocked output byte stream, abstracted over the
/// destination of the output.
///
/// Since it is unbuffered, and since many destinations have high per-call
/// overhead, it is often beneficial to wrap this in a [`BufWriter`] or
/// [`LineWriter`].
///
/// TODO: "Unbuffered" here isn't entirely accurate, given how async-std deals
/// with the underlying OS APIs being effectively synchronous. Figure out what
/// to say here.
///
/// [`BufWriter`]: async_std::io::BufWriter
/// [`LineWriter`]: async_std::io::LineWriter
pub struct AsyncStreamWriter {
    resources: WriteResources,
}

/// An unbuffered and unlocked interactive combination input and output stream.
///
/// There is no `file` constructor, even though [`File`] implements both `Read`
/// and `Write`, because normal files are not interactive. However, there is a
/// `char_device` constructor for [character device files].
///
/// TODO: "Unbuffered" here isn't entirely accurate, given how async-std deals
/// with the underlying OS APIs being effectively synchronous. Figure out what
/// to say here.
///
/// [`File`]: async_std::fs::File
/// [character device files]: https://docs.rs/char-device/latest/char_device/struct.CharDevice.html
pub struct AsyncStreamDuplexer {
    resources: DuplexResources,
}

/// Additional resources that need to be held in order to keep the stream live.
#[allow(dead_code)] // TODO
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
#[allow(dead_code)] // TODO
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
#[allow(dead_code)] // TODO
enum DuplexResources {
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    PipeReaderWriter((PipeReader, PipeWriter)),
    StdinStdout((StdinLocker, StdoutLocker)),
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    Child(Child),
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    ChildStdoutStdin((ChildStdout, ChildStdin)),
    #[cfg(feature = "char-device")]
    CharDevice(AsyncCharDevice),
    TcpStream(TcpStream),
    #[cfg(unix)]
    UnixStream(UnixStream),
    #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
    SocketpairStream(AsyncSocketpairStream),
    #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
    SocketedThreadFunc(
        Option<(
            AsyncSocketpairStream,
            JoinHandle<io::Result<AsyncSocketpairStream>>,
        )>,
    ),
    #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
    SocketedThread(
        Option<(
            AsyncSocketpairStream,
            JoinHandle<io::Result<Box<dyn HalfDuplex + Send>>>,
        )>,
    ),
    #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
    SocketedThreadReadReady(
        Option<(
            AsyncSocketpairStream,
            JoinHandle<io::Result<Box<dyn HalfDuplexReadReady + Send>>>,
        )>,
    ),
}

impl AsyncStreamReader {
    /// Read from standard input.
    ///
    /// Unlike [`async_std::io::stdin`], this `stdin` returns a stream which is
    /// unbuffered and unlocked.
    ///
    /// This acquires a [`async_std::io::StdinLock`] (in a non-recursive way) to
    /// prevent accesses to `async_std::io::Stdin` while this is live, and fails if a
    /// `AsyncStreamReader` or `AsyncStreamDuplexer` for standard input already exists.
    #[inline]
    pub fn stdin() -> io::Result<Self> {
        todo!("async stdin")
    }

    /// Read from an open file, taking ownership of it.
    ///
    /// This method can be passed a [`async_std::fs::File`] or similar `File` types.
    #[inline]
    #[must_use]
    pub fn file<Filelike: IntoUnsafeFile + Read + Write + Seek>(filelike: Filelike) -> Self {
        // Safety: We don't implement `From`/`Into` to allow the inner `File`
        // to be extracted, so we don't need to worry that we're granting
        // ambient authorities here.
        Self::_file(File::from_filelike(filelike))
    }

    #[inline]
    #[must_use]
    fn _file(file: File) -> Self {
        Self::handle(ReadResources::File(file))
    }

    /// Read from an open TCP stream, taking ownership of it.
    ///
    /// This method can be passed a [`async_std::net::TcpStream`] or similar
    /// `TcpStream` types.
    #[inline]
    #[must_use]
    pub fn tcp_stream<Socketlike: IntoUnsafeSocket>(socketlike: Socketlike) -> Self {
        Self::_tcp_stream(TcpStream::from_socketlike(socketlike))
    }

    #[inline]
    #[must_use]
    fn _tcp_stream(tcp_stream: TcpStream) -> Self {
        // Safety: We don't implement `From`/`Into` to allow the inner
        // `TcpStream` to be extracted, so we don't need to worry that
        // we're granting ambient authorities here.
        Self::handle(ReadResources::TcpStream(tcp_stream))
    }

    /// Read from an open Unix-domain socket, taking ownership of it.
    #[cfg(unix)]
    #[inline]
    #[must_use]
    pub fn unix_stream(unix_stream: UnixStream) -> Self {
        Self::handle(ReadResources::UnixStream(unix_stream))
    }

    /// Read from the reading end of an open pipe, taking ownership of it.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    #[inline]
    #[must_use]
    pub fn pipe_reader(_pipe_reader: PipeReader) -> Self {
        todo!("async pipe reader")
    }

    /// Spawn the given command and read from its standard output.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    pub fn read_from_command(_command: Command) -> io::Result<Self> {
        todo!("async command read")
    }

    /// Read from a child process' standard output, taking ownership of it.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    #[inline]
    #[must_use]
    pub fn child_stdout(_child_stdout: ChildStdout) -> Self {
        todo!("async child stdout")
    }

    /// Read from a child process' standard error, taking ownership of it.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    #[inline]
    #[must_use]
    pub fn child_stderr(_child_stderr: ChildStderr) -> Self {
        todo!("async child stderr")
    }

    /// Read from a boxed `Read` implementation, taking ownership of it. This
    /// works by creating a new thread to read the data and write it through a
    /// pipe.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    pub fn piped_thread(_boxed_read: Box<dyn Read + Send>) -> io::Result<Self> {
        todo!("async piped_thread reader")
    }

    /// Read from the given string.
    #[inline]
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    pub fn str<S: AsRef<str>>(s: S) -> io::Result<Self> {
        Self::bytes(s.as_ref().as_bytes())
    }

    /// Read from the given bytes.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    pub fn bytes(_bytes: &[u8]) -> io::Result<Self> {
        todo!("async bytes")
    }

    #[inline]
    #[must_use]
    fn handle(resources: ReadResources) -> Self {
        Self { resources }
    }
}

impl AsyncStreamWriter {
    /// Write to standard output.
    ///
    /// Unlike [`async_std::io::stdout`], this `stdout` returns a stream which is
    /// unbuffered and unlocked.
    ///
    /// This acquires a [`async_std::io::StdoutLock`] (in a non-recursive way) to
    /// prevent accesses to `async_std::io::Stdout` while this is live, and fails if
    /// a `AsyncStreamWriter` or `AsyncStreamDuplexer` for standard output already
    /// exists.
    #[inline]
    pub fn stdout() -> io::Result<Self> {
        todo!("async stdout")
    }

    /// Write to an open file, taking ownership of it.
    ///
    /// This method can be passed a [`async_std::fs::File`] or similar `File` types.
    #[inline]
    #[must_use]
    pub fn file<Filelike: IntoUnsafeFile + Read + Write + Seek>(filelike: Filelike) -> Self {
        // Safety: We don't implement `From`/`Into` to allow the inner `File`
        // to be extracted, so we don't need to worry that we're granting
        // ambient authorities here.
        Self::_file(File::from_filelike(filelike))
    }

    #[inline]
    #[must_use]
    fn _file(file: File) -> Self {
        Self::handle(WriteResources::File(file))
    }

    /// Write to an open TCP stream, taking ownership of it.
    ///
    /// This method can be passed a [`async_std::net::TcpStream`] or similar
    /// `TcpStream` types.
    #[inline]
    #[must_use]
    pub fn tcp_stream<Socketlike: IntoUnsafeSocket>(socketlike: Socketlike) -> Self {
        // Safety: We don't implement `From`/`Into` to allow the inner
        // `TcpStream` to be extracted, so we don't need to worry that we're
        // granting ambient authorities here.
        Self::_tcp_stream(TcpStream::from_socketlike(socketlike))
    }

    #[inline]
    #[must_use]
    fn _tcp_stream(tcp_stream: TcpStream) -> Self {
        Self::handle(WriteResources::TcpStream(tcp_stream))
    }

    /// Write to an open Unix-domain stream, taking ownership of it.
    #[cfg(unix)]
    #[inline]
    #[must_use]
    pub fn unix_stream(unix_stream: UnixStream) -> Self {
        Self::handle(WriteResources::UnixStream(unix_stream))
    }

    /// Write to the writing end of an open pipe, taking ownership of it.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    #[inline]
    #[must_use]
    pub fn pipe_writer(_pipe_writer: PipeWriter) -> Self {
        todo!("async pipe writer")
    }

    /// Spawn the given command and write to its standard input. Its standard
    /// output is redirected to `Stdio::null()`.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    pub fn write_to_command(_command: Command) -> io::Result<Self> {
        todo!("async command write")
    }

    /// Write to the given child standard input, taking ownership of it.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    #[inline]
    #[must_use]
    pub fn child_stdin(_child_stdin: ChildStdin) -> Self {
        todo!("async child stdin")
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
    pub fn piped_thread(_boxed_write: Box<dyn Write + Send>) -> io::Result<Self> {
        todo!("async piped_thread writer")
    }

    /// Write to the null device, which ignores all data.
    pub async fn null() -> io::Result<Self> {
        #[cfg(not(windows))]
        {
            Ok(Self::file(File::create("/dev/null").await?))
        }

        #[cfg(windows)]
        {
            Ok(Self::file(File::create("nul").await?))
        }
    }

    #[inline]
    fn handle(resources: WriteResources) -> Self {
        Self { resources }
    }
}

impl AsyncStreamDuplexer {
    /// Duplex with stdin and stdout, taking ownership of them.
    ///
    /// Unlike [`async_std::io::stdin`] and [`async_std::io::stdout`], this `stdin_stdout`
    /// returns a stream which is unbuffered and unlocked.
    ///
    /// This acquires a [`async_std::io::StdinLock`] and a [`async_std::io::StdoutLock`]
    /// (in non-recursive ways) to prevent accesses to [`async_std::io::Stdin`] and
    /// [`async_std::io::Stdout`] while this is live, and fails if a `AsyncStreamReader`
    /// for standard input, a `AsyncStreamWriter` for standard output, or a
    /// `AsyncStreamDuplexer` for standard input and standard output already exist.
    #[inline]
    pub fn stdin_stdout() -> io::Result<Self> {
        todo!("async stdin_stdout")
    }

    /// Duplex with an open character device, taking ownership of it.
    #[cfg(feature = "char-device")]
    #[inline]
    #[must_use]
    pub fn char_device(char_device: AsyncCharDevice) -> Self {
        Self::handle(DuplexResources::CharDevice(char_device))
    }

    /// Duplex with an open TCP stream, taking ownership of it.
    ///
    /// This method can be passed a [`async_std::net::TcpStream`] or similar
    /// `TcpStream` types.
    #[inline]
    #[must_use]
    pub fn tcp_stream<Socketlike: IntoUnsafeSocket>(socketlike: Socketlike) -> Self {
        Self::_tcp_stream(TcpStream::from_socketlike(socketlike))
    }

    #[inline]
    #[must_use]
    fn _tcp_stream(tcp_stream: TcpStream) -> Self {
        // Safety: We don't implement `From`/`Into` to allow the inner
        // `TcpStream` to be extracted, so we don't need to worry that
        // we're granting ambient authorities here.
        Self::handle(DuplexResources::TcpStream(tcp_stream))
    }

    /// Duplex with an open Unix-domain stream, taking ownership of it.
    #[cfg(unix)]
    #[must_use]
    pub fn unix_stream(unix_stream: UnixStream) -> Self {
        Self::handle(DuplexResources::UnixStream(unix_stream))
    }

    /// Duplex with a pair of pipe streams, taking ownership of them.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    #[inline]
    #[must_use]
    pub fn pipe_reader_writer(_pipe_reader: PipeReader, _pipe_writer: PipeWriter) -> Self {
        todo!("async pipe reader/writer")
    }

    /// Duplex with one end of a socketpair stream, taking ownership of it.
    #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
    #[must_use]
    pub fn socketpair_stream(stream: AsyncSocketpairStream) -> Self {
        Self::handle(DuplexResources::SocketpairStream(stream))
    }

    /// Spawn the given command and duplex with its standard input and output.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    pub fn duplex_with_command(_command: Command) -> io::Result<Self> {
        todo!("async command duplex")
    }

    /// Duplex with a child process' stdout and stdin, taking ownership of
    /// them.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    #[inline]
    #[must_use]
    pub fn child_stdout_stdin(_child_stdout: ChildStdout, _child_stdin: ChildStdin) -> Self {
        todo!("async child stdin/stdout")
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
    pub fn socketed_thread_read_first(
        _boxed_duplex: Box<dyn HalfDuplex + Send>,
    ) -> io::Result<Self> {
        todo!("async socketed_thread_read_first")
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
    pub fn socketed_thread_write_first(
        _boxed_duplex: Box<dyn HalfDuplex + Send>,
    ) -> io::Result<Self> {
        todo!("async socketed_thread_write_first")
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
    pub fn socketed_thread(_boxed_duplex: Box<dyn HalfDuplexReadReady + Send>) -> io::Result<Self> {
        todo!("async socketed_thread")
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
    pub fn socketed_thread_func(
        _func: Box<dyn Send + FnOnce(AsyncSocketpairStream) -> io::Result<AsyncSocketpairStream>>,
    ) -> io::Result<Self> {
        todo!("async socketed_thread_func")
    }

    #[inline]
    #[must_use]
    fn handle(resources: DuplexResources) -> Self {
        Self { resources }
    }
}

impl Read for AsyncStreamReader {
    #[inline]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        match &mut self.resources {
            ReadResources::File(file) => Pin::new(file).poll_read(cx, buf),
            ReadResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).poll_read(cx, buf),
            #[cfg(unix)]
            ReadResources::UnixStream(unix_stream) => Pin::new(unix_stream).poll_read(cx, buf),
            ReadResources::PipeReader(_pipe_reader) => todo!("async pipe read"),
            ReadResources::Stdin(_stdin) => todo!("async stdin read"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::PipedThread(_piped_thread) => todo!("async piped_thread read"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::Child(_child) => todo!("async child read"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::ChildStdout(_child_stdout) => todo!("async child stdout read"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::ChildStderr(_child_stderr) => todo!("async child stderr read"),
        }
    }

    #[inline]
    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        match &mut self.resources {
            ReadResources::File(file) => Pin::new(file).poll_read_vectored(cx, bufs),
            ReadResources::TcpStream(tcp_stream) => {
                Pin::new(tcp_stream).poll_read_vectored(cx, bufs)
            }
            #[cfg(unix)]
            ReadResources::UnixStream(unix_stream) => {
                Pin::new(unix_stream).poll_read_vectored(cx, bufs)
            }
            ReadResources::PipeReader(_pipe_reader) => todo!("async pipe read"),
            ReadResources::Stdin(_stdin) => todo!("async stdin read"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::PipedThread(_piped_thread) => todo!("async piped_thread read"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::Child(_child) => todo!("async child read"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::ChildStdout(_child_stdout) => todo!("async child stdout read"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::ChildStderr(_child_stderr) => todo!("async child stderr read"),
        }
    }
}

/* // TODO
impl Peek for AsyncStreamReader {
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

impl ReadReady for AsyncStreamReader {
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
*/

impl Write for AsyncStreamWriter {
    #[inline]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &mut self.resources {
            WriteResources::File(file) => Pin::new(file).poll_write(cx, buf),
            WriteResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).poll_write(cx, buf),
            #[cfg(unix)]
            WriteResources::UnixStream(unix_stream) => Pin::new(unix_stream).poll_write(cx, buf),
            WriteResources::PipeWriter(_pipe_writer) => todo!("async pipe write"),
            WriteResources::Stdout(_stdout) => todo!("async stdout write"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::PipedThread(_piped_thread) => todo!("async piped_thread write"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::Child(_child) => todo!("async child write"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::ChildStdin(_child_stdin) => todo!("async child stdin write"),
        }
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match &mut self.resources {
            WriteResources::File(file) => Pin::new(file).poll_write_vectored(cx, bufs),
            WriteResources::TcpStream(tcp_stream) => {
                Pin::new(tcp_stream).poll_write_vectored(cx, bufs)
            }
            #[cfg(unix)]
            WriteResources::UnixStream(unix_stream) => {
                Pin::new(unix_stream).poll_write_vectored(cx, bufs)
            }
            WriteResources::PipeWriter(_pipe_writer) => todo!("async pipe write"),
            WriteResources::Stdout(_stdout) => todo!("async stdout write"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::PipedThread(_piped_thread) => todo!("async piped_thread write"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::Child(_child) => todo!("async child write"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::ChildStdin(_child_stdin) => todo!("async child stdin write"),
        }
    }

    #[inline]
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.resources {
            WriteResources::File(file) => Pin::new(file).poll_flush(cx),
            WriteResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).poll_flush(cx),
            #[cfg(unix)]
            WriteResources::UnixStream(unix_stream) => Pin::new(unix_stream).poll_flush(cx),
            WriteResources::PipeWriter(_pipe_writer) => todo!("async pipe flush"),
            WriteResources::Stdout(_stdout) => todo!("async stdout flush"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::PipedThread(_piped_thread) => todo!("async piped_thread flush"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::Child(_child) => todo!("async child flush"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::ChildStdin(_child_stdin) => todo!("async child stdin flush"),
        }
    }

    #[inline]
    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.resources {
            WriteResources::File(file) => Pin::new(file).poll_close(cx),
            WriteResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).poll_close(cx),
            #[cfg(unix)]
            WriteResources::UnixStream(unix_stream) => Pin::new(unix_stream).poll_close(cx),
            WriteResources::PipeWriter(_pipe_writer) => todo!("async pipe close"),
            WriteResources::Stdout(_stdout) => todo!("async stdout close"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::PipedThread(_piped_thread) => todo!("async piped_thread close"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::Child(_child) => todo!("async child close"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::ChildStdin(_child_stdin) => todo!("async child stdin close"),
        }
    }
}

impl Read for AsyncStreamDuplexer {
    #[inline]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        match &mut self.resources {
            DuplexResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).poll_read(cx, buf),
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => Pin::new(unix_stream).poll_read(cx, buf),
            DuplexResources::StdinStdout(_stdin_stdout) => todo!("async stdin_stdout read"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("async child read"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_child_stdout_stdin) => {
                todo!("async child stdout/stdin read")
            }
            #[cfg(feature = "char-device")]
            DuplexResources::CharDevice(char_device) => Pin::new(char_device).poll_read(cx, buf),
            #[cfg(feature = "socketpair")]
            DuplexResources::SocketpairStream(socketpair_stream) => {
                Pin::new(socketpair_stream).poll_read(cx, buf)
            }
            DuplexResources::PipeReaderWriter(_)
            | DuplexResources::SocketedThreadFunc(_)
            | DuplexResources::SocketedThread(_)
            | DuplexResources::SocketedThreadReadReady(_) => todo!("async duplex resources"),
        }
    }

    #[inline]
    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        match &mut self.resources {
            DuplexResources::TcpStream(tcp_stream) => {
                Pin::new(tcp_stream).poll_read_vectored(cx, bufs)
            }
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => {
                Pin::new(unix_stream).poll_read_vectored(cx, bufs)
            }
            DuplexResources::StdinStdout(_stdin_stdout) => todo!("async stdin_stdout read"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("async child read"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_child_stdout_stdin) => {
                todo!("async child stdout/stdin read")
            }
            #[cfg(feature = "char-device")]
            DuplexResources::CharDevice(char_device) => {
                Pin::new(char_device).poll_read_vectored(cx, bufs)
            }
            #[cfg(feature = "socketpair")]
            DuplexResources::SocketpairStream(socketpair_stream) => {
                Pin::new(socketpair_stream).poll_read_vectored(cx, bufs)
            }
            DuplexResources::PipeReaderWriter(_)
            | DuplexResources::SocketedThreadFunc(_)
            | DuplexResources::SocketedThread(_)
            | DuplexResources::SocketedThreadReadReady(_) => todo!("async duplex resources"),
        }
    }
}

/* // TODO
impl Peek for AsyncStreamDuplexer {
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

impl ReadReady for AsyncStreamDuplexer {
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
*/

impl Write for AsyncStreamDuplexer {
    #[inline]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &mut self.resources {
            DuplexResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).poll_write(cx, buf),
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => Pin::new(unix_stream).poll_write(cx, buf),
            DuplexResources::StdinStdout(_stdin_stdout) => todo!("async stdout write"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("async child write"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_stdout_stdin) => {
                todo!("async child stdout/stdin write")
            }
            #[cfg(feature = "char-device")]
            DuplexResources::CharDevice(char_device) => Pin::new(char_device).poll_write(cx, buf),
            #[cfg(feature = "socketpair")]
            DuplexResources::SocketpairStream(socketpair_stream) => {
                Pin::new(socketpair_stream).poll_write(cx, buf)
            }
            DuplexResources::PipeReaderWriter(_)
            | DuplexResources::SocketedThreadFunc(_)
            | DuplexResources::SocketedThread(_)
            | DuplexResources::SocketedThreadReadReady(_) => todo!("async duplex resources"),
        }
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match &mut self.resources {
            DuplexResources::TcpStream(tcp_stream) => {
                Pin::new(tcp_stream).poll_write_vectored(cx, bufs)
            }
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => {
                Pin::new(unix_stream).poll_write_vectored(cx, bufs)
            }
            DuplexResources::StdinStdout(_stdin_stdout) => todo!("async stdout write"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("async child write"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_stdout_stdin) => {
                todo!("async child stdout/stdin write")
            }
            #[cfg(feature = "char-device")]
            DuplexResources::CharDevice(char_device) => {
                Pin::new(char_device).poll_write_vectored(cx, bufs)
            }
            #[cfg(feature = "socketpair")]
            DuplexResources::SocketpairStream(socketpair_stream) => {
                Pin::new(socketpair_stream).poll_write_vectored(cx, bufs)
            }
            DuplexResources::PipeReaderWriter(_)
            | DuplexResources::SocketedThreadFunc(_)
            | DuplexResources::SocketedThread(_)
            | DuplexResources::SocketedThreadReadReady(_) => todo!("async duplex resources"),
        }
    }

    #[inline]
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.resources {
            DuplexResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).poll_flush(cx),
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => Pin::new(unix_stream).poll_flush(cx),
            DuplexResources::StdinStdout(_stdin_stdout) => todo!("async stdout flush"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("async child flush"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_child_stdout_stdin) => {
                todo!("async child stdout/stdin flush")
            }
            #[cfg(feature = "char-device")]
            DuplexResources::CharDevice(char_device) => Pin::new(char_device).poll_flush(cx),
            #[cfg(feature = "socketpair")]
            DuplexResources::SocketpairStream(socketpair_stream) => {
                Pin::new(socketpair_stream).poll_flush(cx)
            }
            DuplexResources::PipeReaderWriter(_)
            | DuplexResources::SocketedThreadFunc(_)
            | DuplexResources::SocketedThread(_)
            | DuplexResources::SocketedThreadReadReady(_) => todo!("async duplex resources"),
        }
    }

    #[inline]
    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.resources {
            DuplexResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).poll_close(cx),
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => Pin::new(unix_stream).poll_close(cx),
            DuplexResources::StdinStdout(_stdin_stdout) => todo!("async stdout close"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("async child close"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_child_stdout_stdin) => {
                todo!("async child stdout/stdin close")
            }
            #[cfg(feature = "char-device")]
            DuplexResources::CharDevice(char_device) => Pin::new(char_device).poll_close(cx),
            #[cfg(feature = "socketpair")]
            DuplexResources::SocketpairStream(socketpair_stream) => {
                Pin::new(socketpair_stream).poll_close(cx)
            }
            DuplexResources::PipeReaderWriter(_)
            | DuplexResources::SocketedThreadFunc(_)
            | DuplexResources::SocketedThread(_)
            | DuplexResources::SocketedThreadReadReady(_) => todo!("async duplex resources"),
        }
    }
}

impl Duplex for AsyncStreamDuplexer {}

#[cfg(not(windows))]
impl AsRawFd for AsyncStreamReader {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        match &self.resources {
            ReadResources::File(file) => file.as_raw_fd(),
            ReadResources::TcpStream(tcp_stream) => tcp_stream.as_raw_fd(),
            #[cfg(unix)]
            ReadResources::UnixStream(unix_stream) => unix_stream.as_raw_fd(),
            ReadResources::PipeReader(_pipe_reader) => todo!("async pipe as_raw_fd"),
            ReadResources::Stdin(_stdin) => todo!("async stdin as_raw_fd"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::PipedThread(_piped_thread) => todo!("async piped_thread as_raw_fd"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::Child(_child) => todo!("async child as_raw_fd"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::ChildStdout(_child_stdout) => todo!("async child stdout as_raw_fd"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::ChildStderr(_child_stderr) => todo!("async child stderr as_raw_fd"),
        }
    }
}

#[cfg(not(windows))]
impl AsRawFd for AsyncStreamWriter {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        match &self.resources {
            WriteResources::File(file) => Pin::new(file).as_raw_fd(),
            WriteResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).as_raw_fd(),
            #[cfg(unix)]
            WriteResources::UnixStream(unix_stream) => Pin::new(unix_stream).as_raw_fd(),
            WriteResources::PipeWriter(_pipe_writer) => todo!("async pipe as_raw_fd"),
            WriteResources::Stdout(_stdout) => todo!("async stdout as_raw_fd"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::PipedThread(_piped_thread) => todo!("async piped_thread as_raw_fd"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::Child(_child) => todo!("async child as_raw_fd"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::ChildStdin(_child_stdin) => todo!("async child stdin as_raw_fd"),
        }
    }
}

#[cfg(not(windows))]
impl AsRawReadWriteFd for AsyncStreamDuplexer {
    #[inline]
    fn as_raw_read_fd(&self) -> RawFd {
        match &self.resources {
            DuplexResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).as_raw_fd(),
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => Pin::new(unix_stream).as_raw_fd(),
            DuplexResources::StdinStdout(_stdin_stdout) => todo!("async stdout as_raw_read_fd"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("async child as_raw_read_fd"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_stdout_stdin) => {
                todo!("async child stdout/stdin as_raw_read_fd")
            }
            #[cfg(feature = "char-device")]
            DuplexResources::CharDevice(char_device) => Pin::new(char_device).as_raw_fd(),
            #[cfg(feature = "socketpair")]
            DuplexResources::SocketpairStream(socketpair_stream) => {
                Pin::new(socketpair_stream).as_raw_fd()
            }
            DuplexResources::PipeReaderWriter(_)
            | DuplexResources::SocketedThreadFunc(_)
            | DuplexResources::SocketedThread(_)
            | DuplexResources::SocketedThreadReadReady(_) => todo!("async duplex resources"),
        }
    }

    #[inline]
    fn as_raw_write_fd(&self) -> RawFd {
        match &self.resources {
            DuplexResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).as_raw_fd(),
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => Pin::new(unix_stream).as_raw_fd(),
            DuplexResources::StdinStdout(_stdin_stdout) => todo!("async stdout as_raw_write_fd"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("async child as_raw_write_fd"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_stdout_stdin) => {
                todo!("async child stdout/stdin as_raw_write_fd")
            }
            #[cfg(feature = "char-device")]
            DuplexResources::CharDevice(char_device) => Pin::new(char_device).as_raw_fd(),
            #[cfg(feature = "socketpair")]
            DuplexResources::SocketpairStream(socketpair_stream) => {
                Pin::new(socketpair_stream).as_raw_fd()
            }
            DuplexResources::PipeReaderWriter(_)
            | DuplexResources::SocketedThreadFunc(_)
            | DuplexResources::SocketedThread(_)
            | DuplexResources::SocketedThreadReadReady(_) => todo!("async duplex resources"),
        }
    }
}

#[cfg(windows)]
impl AsRawHandleOrSocket for AsyncStreamReader {
    #[inline]
    fn as_raw_handle_or_socket(&self) -> RawHandleOrSocket {
        match &self.resources {
            ReadResources::File(file) => file.as_raw_handle_or_socket(),
            ReadResources::TcpStream(tcp_stream) => tcp_stream.as_raw_handle_or_socket(),
            #[cfg(unix)]
            ReadResources::UnixStream(unix_stream) => unix_stream.as_raw_handle_or_socket(),
            ReadResources::PipeReader(_pipe_reader) => todo!("async pipe as_raw_handle_or_socket"),
            ReadResources::Stdin(_stdin) => todo!("async stdin as_raw_handle_or_socket"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::PipedThread(_piped_thread) => {
                todo!("async piped_thread as_raw_handle_or_socket")
            }
            #[cfg(not(target_os = "wasi"))]
            ReadResources::Child(_child) => todo!("async child as_raw_handle_or_socket"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::ChildStdout(_child_stdout) => {
                todo!("async child stdout as_raw_handle_or_socket")
            }
            #[cfg(not(target_os = "wasi"))]
            ReadResources::ChildStderr(_child_stderr) => {
                todo!("async child stderr as_raw_handle_or_socket")
            }
        }
    }
}

#[cfg(windows)]
impl AsRawHandleOrSocket for AsyncStreamWriter {
    #[inline]
    fn as_raw_handle_or_socket(&self) -> RawHandleOrSocket {
        match &self.resources {
            WriteResources::File(file) => Pin::new(file).as_raw_handle_or_socket(),
            WriteResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).as_raw_handle_or_socket(),
            #[cfg(unix)]
            WriteResources::UnixStream(unix_stream) => {
                Pin::new(unix_stream).as_raw_handle_or_socket()
            }
            WriteResources::PipeWriter(_pipe_writer) => todo!("async pipe as_raw_handle_or_socket"),
            WriteResources::Stdout(_stdout) => todo!("async stdout as_raw_handle_or_socket"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::PipedThread(_piped_thread) => {
                todo!("async piped_thread as_raw_handle_or_socket")
            }
            #[cfg(not(target_os = "wasi"))]
            WriteResources::Child(_child) => todo!("async child as_raw_handle_or_socket"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::ChildStdin(_child_stdin) => {
                todo!("async child stdin as_raw_handle_or_socket")
            }
        }
    }
}

#[cfg(windows)]
impl AsRawReadWriteHandleOrSocket for AsyncStreamDuplexer {
    #[inline]
    fn as_raw_read_handle_or_socket(&self) -> RawHandleOrSocket {
        match &self.resources {
            DuplexResources::TcpStream(tcp_stream) => {
                Pin::new(tcp_stream).as_raw_handle_or_socket()
            }
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => {
                Pin::new(unix_stream).as_raw_handle_or_socket()
            }
            DuplexResources::StdinStdout(_stdin_stdout) => {
                todo!("async stdout as_raw_handle_or_socket")
            }
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("async child as_raw_handle_or_socket"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_stdout_stdin) => {
                todo!("async child stdout/stdin as_raw_handle_or_socket")
            }
            #[cfg(feature = "char-device")]
            DuplexResources::CharDevice(char_device) => {
                Pin::new(char_device).as_raw_handle_or_socket()
            }
            #[cfg(feature = "socketpair")]
            DuplexResources::SocketpairStream(socketpair_stream) => {
                Pin::new(socketpair_stream).as_raw_handle_or_socket()
            }
            DuplexResources::PipeReaderWriter(_)
            | DuplexResources::SocketedThreadFunc(_)
            | DuplexResources::SocketedThread(_)
            | DuplexResources::SocketedThreadReadReady(_) => todo!("async duplex resources"),
        }
    }

    #[inline]
    fn as_raw_write_handle_or_socket(&self) -> RawHandleOrSocket {
        match &self.resources {
            DuplexResources::TcpStream(tcp_stream) => {
                Pin::new(tcp_stream).as_raw_handle_or_socket()
            }
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => {
                Pin::new(unix_stream).as_raw_handle_or_socket()
            }
            DuplexResources::StdinStdout(_stdin_stdout) => {
                todo!("async stdout as_raw_handle_or_socket")
            }
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("async child as_raw_handle_or_socket"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_stdout_stdin) => {
                todo!("async child stdout/stdin as_raw_handle_or_socket")
            }
            #[cfg(feature = "char-device")]
            DuplexResources::CharDevice(char_device) => {
                Pin::new(char_device).as_raw_handle_or_socket()
            }
            #[cfg(feature = "socketpair")]
            DuplexResources::SocketpairStream(socketpair_stream) => {
                Pin::new(socketpair_stream).as_raw_handle_or_socket()
            }
            DuplexResources::PipeReaderWriter(_)
            | DuplexResources::SocketedThreadFunc(_)
            | DuplexResources::SocketedThread(_)
            | DuplexResources::SocketedThreadReadReady(_) => todo!("async duplex resources"),
        }
    }
}

// Safety: AsyncStreamReader owns its handle.
unsafe impl OwnsRaw for AsyncStreamReader {}

// Safety: AsyncStreamWriter owns its handle.
unsafe impl OwnsRaw for AsyncStreamWriter {}

// Safety: AsyncStreamDuplexer owns its handle.
unsafe impl OwnsRaw for AsyncStreamDuplexer {}

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

impl Debug for AsyncStreamReader {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Just print the fd number; don't try to print the path or any
        // information about it, because this information is otherwise
        // unavailable to safe Rust code.
        f.debug_struct("AsyncStreamReader")
            .field("unsafe_handle", &self.as_unsafe_handle())
            .finish()
    }
}

impl Debug for AsyncStreamWriter {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Just print the fd number; don't try to print the path or any
        // information about it, because this information is otherwise
        // unavailable to safe Rust code.
        f.debug_struct("AsyncStreamWriter")
            .field("unsafe_handle", &self.as_unsafe_handle())
            .finish()
    }
}

impl Debug for AsyncStreamDuplexer {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Just print the fd numbers; don't try to print the path or any
        // information about it, because this information is otherwise
        // unavailable to safe Rust code.
        f.debug_struct("AsyncStreamDuplexer")
            .field("unsafe_readable", &self.as_unsafe_read_handle())
            .field("unsafe_writeable", &self.as_unsafe_write_handle())
            .finish()
    }
}

/// A trait that combines [`HalfDuplex`] and [`ReadReady`]. Implemented via
/// blanket implementation.
#[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
pub trait HalfDuplexReadReady: HalfDuplex + ReadReady {}

#[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
impl<T: HalfDuplex + ReadReady> HalfDuplexReadReady for T {}
