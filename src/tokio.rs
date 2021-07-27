use crate::lockers::{StdinLocker, StdoutLocker};
#[cfg(feature = "char-device")]
use char_device::TokioCharDevice;
use duplex::Duplex;
use io_lifetimes::{FromFilelike, IntoFilelike};
#[cfg(target_os = "wasi")]
use std::os::wasi::io::{AsRawFd, RawFd};
use std::{
    fmt::{self, Debug},
    io::IoSlice,
    pin::Pin,
    task::{Context, Poll},
};
use system_interface::io::ReadReady;
use tokio::{
    fs::File,
    io::{self, AsyncRead, AsyncSeek, AsyncWrite, ReadBuf},
    net::TcpStream,
};
#[cfg(windows)]
use unsafe_io::os::windows::{
    AsHandleOrSocket, AsRawHandleOrSocket, AsRawReadWriteHandleOrSocket, AsReadWriteHandleOrSocket,
    BorrowedHandleOrSocket, RawHandleOrSocket,
};
use unsafe_io::{AsRawGrip, AsRawReadWriteGrip};
#[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
use {duplex::HalfDuplex, socketpair::TokioSocketpairStream};
#[cfg(not(windows))]
use {
    io_lifetimes::{AsFd, BorrowedFd},
    unsafe_io::os::posish::{AsRawReadWriteFd, AsReadWriteFd},
};
#[cfg(not(target_os = "wasi"))]
use {
    os_pipe::{PipeReader, PipeWriter},
    std::{
        process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
        thread::JoinHandle,
    },
};
#[cfg(unix)]
use {
    std::os::unix::io::{AsRawFd, RawFd},
    tokio::net::UnixStream,
};

/// An unbuffered and unlocked input byte stream, abstracted over the source of
/// the input.
///
/// Since it is unbuffered, and since many input sources have high per-call
/// overhead, it is often beneficial to wrap this in a [`BufReader`].
///
/// TODO: "Unbuffered" here isn't entirely accurate, given how tokio deals
/// with the underlying OS APIs being effectively synchronous. Figure out what
/// to say here.
///
/// [`BufReader`]: tokio::io::BufReader
pub struct TokioStreamReader {
    resources: ReadResources,
}

/// An unbuffered and unlocked output byte stream, abstracted over the
/// destination of the output.
///
/// Since it is unbuffered, and since many destinations have high per-call
/// overhead, it is often beneficial to wrap this in a [`BufWriter`] or
/// [`LineWriter`].
///
/// TODO: "Unbuffered" here isn't entirely accurate, given how tokio deals
/// with the underlying OS APIs being effectively synchronous. Figure out what
/// to say here.
///
/// [`BufWriter`]: tokio::io::BufWriter
/// [`LineWriter`]: tokio::io::LineWriter
pub struct TokioStreamWriter {
    resources: WriteResources,
}

/// An unbuffered and unlocked interactive combination input and output stream.
///
/// There is no `file` constructor, even though [`File`] implements both `Read`
/// and `Write`, because normal files are not interactive. However, there is a
/// `char_device` constructor for [character device files].
///
/// TODO: "Unbuffered" here isn't entirely accurate, given how tokio deals
/// with the underlying OS APIs being effectively synchronous. Figure out what
/// to say here.
///
/// [`File`]: tokio::fs::File
/// [character device files]: https://docs.rs/char-device/latest/char_device/struct.CharDevice.html
pub struct TokioStreamDuplexer {
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
    PipedThread(
        Option<(
            PipeWriter,
            JoinHandle<io::Result<Box<dyn AsyncWrite + Send>>>,
        )>,
    ),
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
    CharDevice(TokioCharDevice),
    TcpStream(TcpStream),
    #[cfg(unix)]
    UnixStream(UnixStream),
    #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
    SocketpairStream(TokioSocketpairStream),
    #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
    SocketedThreadFunc(
        Option<(
            TokioSocketpairStream,
            JoinHandle<io::Result<TokioSocketpairStream>>,
        )>,
    ),
    #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
    SocketedThread(
        Option<(
            TokioSocketpairStream,
            JoinHandle<io::Result<Box<dyn HalfDuplex + Send>>>,
        )>,
    ),
    #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
    SocketedThreadReadReady(
        Option<(
            TokioSocketpairStream,
            JoinHandle<io::Result<Box<dyn HalfDuplexReadReady + Send>>>,
        )>,
    ),
}

impl TokioStreamReader {
    /// Read from standard input.
    ///
    /// Unlike [`tokio::io::stdin`], this `stdin` returns a stream which is
    /// unbuffered and unlocked.
    ///
    /// This acquires a [`tokio::io::StdinLock`] (in a non-recursive way) to
    /// prevent accesses to `tokio::io::Stdin` while this is live, and fails if a
    /// `TokioStreamReader` or `TokioStreamDuplexer` for standard input already exists.
    #[inline]
    pub fn stdin() -> io::Result<Self> {
        todo!("tokio stdin")
    }

    /// Read from an open file, taking ownership of it.
    ///
    /// This method can be passed a [`tokio::fs::File`] or similar `File` types.
    #[inline]
    #[must_use]
    pub fn file<Filelike: IntoFilelike + AsyncRead + AsyncWrite + AsyncSeek>(
        filelike: Filelike,
    ) -> Self {
        // Safety: We don't implement `From`/`Into` to allow the inner `File`
        // to be extracted, so we don't need to worry that we're granting
        // ambient authorities here.
        Self::_file(File::from_into_filelike(filelike))
    }

    #[inline]
    #[must_use]
    fn _file(file: File) -> Self {
        Self::handle(ReadResources::File(file))
    }

    /// Read from an open TCP stream, taking ownership of it.
    ///
    /// This method can be passed a [`tokio::net::TcpStream`] or similar
    /// `TcpStream` types.
    #[inline]
    #[must_use]
    pub fn tcp_stream(tcp_stream: TcpStream) -> Self {
        Self::_tcp_stream(tcp_stream)
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
        todo!("tokio pipe reader")
    }

    /// Spawn the given command and read from its standard output.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    pub fn read_from_command(_command: Command) -> io::Result<Self> {
        todo!("tokio command read")
    }

    /// Read from a child process' standard output, taking ownership of it.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    #[inline]
    #[must_use]
    pub fn child_stdout(_child_stdout: ChildStdout) -> Self {
        todo!("tokio child stdout")
    }

    /// Read from a child process' standard error, taking ownership of it.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    #[inline]
    #[must_use]
    pub fn child_stderr(_child_stderr: ChildStderr) -> Self {
        todo!("tokio child stderr")
    }

    /// Read from a boxed `Read` implementation, taking ownership of it. This
    /// works by creating a new thread to read the data and write it through a
    /// pipe.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    pub fn piped_thread(_boxed_read: Box<dyn AsyncRead + Send>) -> io::Result<Self> {
        todo!("tokio piped_thread reader")
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
        todo!("tokio bytes")
    }

    #[inline]
    #[must_use]
    fn handle(resources: ReadResources) -> Self {
        Self { resources }
    }
}

impl TokioStreamWriter {
    /// Write to standard output.
    ///
    /// Unlike [`tokio::io::stdout`], this `stdout` returns a stream which is
    /// unbuffered and unlocked.
    ///
    /// This acquires a [`tokio::io::StdoutLock`] (in a non-recursive way) to
    /// prevent accesses to `tokio::io::Stdout` while this is live, and fails if
    /// a `TokioStreamWriter` or `TokioStreamDuplexer` for standard output already
    /// exists.
    #[inline]
    pub fn stdout() -> io::Result<Self> {
        todo!("tokio stdout")
    }

    /// Write to an open file, taking ownership of it.
    ///
    /// This method can be passed a [`tokio::fs::File`] or similar `File` types.
    #[inline]
    #[must_use]
    pub fn file<Filelike: IntoFilelike + AsyncRead + AsyncWrite + AsyncSeek>(
        filelike: Filelike,
    ) -> Self {
        // Safety: We don't implement `From`/`Into` to allow the inner `File`
        // to be extracted, so we don't need to worry that we're granting
        // ambient authorities here.
        Self::_file(File::from_into_filelike(filelike))
    }

    #[inline]
    #[must_use]
    fn _file(file: File) -> Self {
        Self::handle(WriteResources::File(file))
    }

    /// Write to an open TCP stream, taking ownership of it.
    ///
    /// This method can be passed a [`tokio::net::TcpStream`] or similar
    /// `TcpStream` types.
    #[inline]
    #[must_use]
    pub fn tcp_stream(tcp_stream: TcpStream) -> Self {
        // Safety: We don't implement `From`/`Into` to allow the inner
        // `TcpStream` to be extracted, so we don't need to worry that we're
        // granting ambient authorities here.
        Self::_tcp_stream(tcp_stream)
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
        todo!("tokio pipe writer")
    }

    /// Spawn the given command and write to its standard input. Its standard
    /// output is redirected to `Stdio::null()`.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    pub fn write_to_command(_command: Command) -> io::Result<Self> {
        todo!("tokio command write")
    }

    /// Write to the given child standard input, taking ownership of it.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    #[inline]
    #[must_use]
    pub fn child_stdin(_child_stdin: ChildStdin) -> Self {
        todo!("tokio child stdin")
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
    pub fn piped_thread(_boxed_write: Box<dyn AsyncWrite + Send>) -> io::Result<Self> {
        todo!("tokio piped_thread writer")
    }

    /// Write to the null device, which ignores all data.
    pub async fn null() -> io::Result<Self> {
        #[cfg(not(windows))]
        {
            Ok(Self::_file(File::create("/dev/null").await?))
        }

        #[cfg(windows)]
        {
            Ok(Self::_file(File::create("nul").await?))
        }
    }

    #[inline]
    fn handle(resources: WriteResources) -> Self {
        Self { resources }
    }
}

impl TokioStreamDuplexer {
    /// Duplex with stdin and stdout, taking ownership of them.
    ///
    /// Unlike [`tokio::io::stdin`] and [`tokio::io::stdout`], this `stdin_stdout`
    /// returns a stream which is unbuffered and unlocked.
    ///
    /// This acquires a [`tokio::io::StdinLock`] and a [`tokio::io::StdoutLock`]
    /// (in non-recursive ways) to prevent accesses to [`tokio::io::Stdin`] and
    /// [`tokio::io::Stdout`] while this is live, and fails if a `TokioStreamReader`
    /// for standard input, a `TokioStreamWriter` for standard output, or a
    /// `TokioStreamDuplexer` for standard input and standard output already exist.
    #[inline]
    pub fn stdin_stdout() -> io::Result<Self> {
        todo!("tokio stdin_stdout")
    }

    /// Duplex with an open character device, taking ownership of it.
    #[cfg(feature = "char-device")]
    #[inline]
    #[must_use]
    pub fn char_device(char_device: TokioCharDevice) -> Self {
        Self::handle(DuplexResources::CharDevice(char_device))
    }

    /// Duplex with an open TCP stream, taking ownership of it.
    ///
    /// This method can be passed a [`tokio::net::TcpStream`] or similar
    /// `TcpStream` types.
    #[inline]
    #[must_use]
    pub fn tcp_stream(tcp_stream: TcpStream) -> Self {
        Self::_tcp_stream(tcp_stream)
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
        todo!("tokio pipe reader/writer")
    }

    /// Duplex with one end of a socketpair stream, taking ownership of it.
    #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
    #[must_use]
    pub fn socketpair_stream(stream: TokioSocketpairStream) -> Self {
        Self::handle(DuplexResources::SocketpairStream(stream))
    }

    /// Spawn the given command and duplex with its standard input and output.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    pub fn duplex_with_command(_command: Command) -> io::Result<Self> {
        todo!("tokio command duplex")
    }

    /// Duplex with a child process' stdout and stdin, taking ownership of
    /// them.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    #[inline]
    #[must_use]
    pub fn child_stdout_stdin(_child_stdout: ChildStdout, _child_stdin: ChildStdin) -> Self {
        todo!("tokio child stdin/stdout")
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
        todo!("tokio socketed_thread_read_first")
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
        todo!("tokio socketed_thread_write_first")
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
        todo!("tokio socketed_thread")
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
        _func: Box<dyn Send + FnOnce(TokioSocketpairStream) -> io::Result<TokioSocketpairStream>>,
    ) -> io::Result<Self> {
        todo!("tokio socketed_thread_func")
    }

    #[inline]
    #[must_use]
    fn handle(resources: DuplexResources) -> Self {
        Self { resources }
    }
}

impl AsyncRead for TokioStreamReader {
    #[inline]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf,
    ) -> Poll<io::Result<()>> {
        match &mut self.resources {
            ReadResources::File(file) => Pin::new(file).poll_read(cx, buf),
            ReadResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).poll_read(cx, buf),
            #[cfg(unix)]
            ReadResources::UnixStream(unix_stream) => Pin::new(unix_stream).poll_read(cx, buf),
            ReadResources::PipeReader(_pipe_reader) => todo!("tokio pipe read"),
            ReadResources::Stdin(_stdin) => todo!("tokio stdin read"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::PipedThread(_piped_thread) => todo!("tokio piped_thread read"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::Child(_child) => todo!("tokio child read"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::ChildStdout(_child_stdout) => todo!("tokio child stdout read"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::ChildStderr(_child_stderr) => todo!("tokio child stderr read"),
        }
    }
}

/* // TODO
impl Peek for TokioStreamReader {
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

impl ReadReady for TokioStreamReader {
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

impl AsyncWrite for TokioStreamWriter {
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
            WriteResources::PipeWriter(_pipe_writer) => todo!("tokio pipe write"),
            WriteResources::Stdout(_stdout) => todo!("tokio stdout write"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::PipedThread(_piped_thread) => todo!("tokio piped_thread write"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::Child(_child) => todo!("tokio child write"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::ChildStdin(_child_stdin) => todo!("tokio child stdin write"),
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
            WriteResources::PipeWriter(_pipe_writer) => todo!("tokio pipe write"),
            WriteResources::Stdout(_stdout) => todo!("tokio stdout write"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::PipedThread(_piped_thread) => todo!("tokio piped_thread write"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::Child(_child) => todo!("tokio child write"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::ChildStdin(_child_stdin) => todo!("tokio child stdin write"),
        }
    }

    #[inline]
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.resources {
            WriteResources::File(file) => Pin::new(file).poll_flush(cx),
            WriteResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).poll_flush(cx),
            #[cfg(unix)]
            WriteResources::UnixStream(unix_stream) => Pin::new(unix_stream).poll_flush(cx),
            WriteResources::PipeWriter(_pipe_writer) => todo!("tokio pipe flush"),
            WriteResources::Stdout(_stdout) => todo!("tokio stdout flush"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::PipedThread(_piped_thread) => todo!("tokio piped_thread flush"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::Child(_child) => todo!("tokio child flush"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::ChildStdin(_child_stdin) => todo!("tokio child stdin flush"),
        }
    }

    #[inline]
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.resources {
            WriteResources::File(file) => Pin::new(file).poll_shutdown(cx),
            WriteResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).poll_shutdown(cx),
            #[cfg(unix)]
            WriteResources::UnixStream(unix_stream) => Pin::new(unix_stream).poll_shutdown(cx),
            WriteResources::PipeWriter(_pipe_writer) => todo!("tokio pipe close"),
            WriteResources::Stdout(_stdout) => todo!("tokio stdout close"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::PipedThread(_piped_thread) => todo!("tokio piped_thread close"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::Child(_child) => todo!("tokio child close"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::ChildStdin(_child_stdin) => todo!("tokio child stdin close"),
        }
    }
}

impl AsyncRead for TokioStreamDuplexer {
    #[inline]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf,
    ) -> Poll<io::Result<()>> {
        match &mut self.resources {
            DuplexResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).poll_read(cx, buf),
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => Pin::new(unix_stream).poll_read(cx, buf),
            DuplexResources::StdinStdout(_stdin_stdout) => todo!("tokio stdin_stdout read"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("tokio child read"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_child_stdout_stdin) => {
                todo!("tokio child stdout/stdin read")
            }
            #[cfg(feature = "char-device")]
            DuplexResources::CharDevice(char_device) => Pin::new(char_device).poll_read(cx, buf),
            #[cfg(feature = "socketpair")]
            DuplexResources::SocketpairStream(socketpair_stream) => {
                Pin::new(socketpair_stream).poll_read(cx, buf)
            }
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::PipeReaderWriter(_) => todo!("tokio duplex resources"),
            #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
            DuplexResources::SocketedThreadFunc(_)
            | DuplexResources::SocketedThread(_)
            | DuplexResources::SocketedThreadReadReady(_) => todo!("tokio duplex resources"),
        }
    }
}

/* // TODO
impl Peek for TokioStreamDuplexer {
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

impl ReadReady for TokioStreamDuplexer {
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

impl AsyncWrite for TokioStreamDuplexer {
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
            DuplexResources::StdinStdout(_stdin_stdout) => todo!("tokio stdout write"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("tokio child write"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_stdout_stdin) => {
                todo!("tokio child stdout/stdin write")
            }
            #[cfg(feature = "char-device")]
            DuplexResources::CharDevice(char_device) => Pin::new(char_device).poll_write(cx, buf),
            #[cfg(feature = "socketpair")]
            DuplexResources::SocketpairStream(socketpair_stream) => {
                Pin::new(socketpair_stream).poll_write(cx, buf)
            }
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::PipeReaderWriter(_) => todo!("tokio duplex resources"),
            #[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
            DuplexResources::SocketedThreadFunc(_)
            | DuplexResources::SocketedThread(_)
            | DuplexResources::SocketedThreadReadReady(_) => todo!("tokio duplex resources"),
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
            DuplexResources::StdinStdout(_stdin_stdout) => todo!("tokio stdout write"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("tokio child write"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_stdout_stdin) => {
                todo!("tokio child stdout/stdin write")
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
            | DuplexResources::SocketedThreadReadReady(_) => todo!("tokio duplex resources"),
        }
    }

    #[inline]
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.resources {
            DuplexResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).poll_flush(cx),
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => Pin::new(unix_stream).poll_flush(cx),
            DuplexResources::StdinStdout(_stdin_stdout) => todo!("tokio stdout flush"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("tokio child flush"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_child_stdout_stdin) => {
                todo!("tokio child stdout/stdin flush")
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
            | DuplexResources::SocketedThreadReadReady(_) => todo!("tokio duplex resources"),
        }
    }

    #[inline]
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.resources {
            DuplexResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).poll_shutdown(cx),
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => Pin::new(unix_stream).poll_shutdown(cx),
            DuplexResources::StdinStdout(_stdin_stdout) => todo!("tokio stdout close"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("tokio child close"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_child_stdout_stdin) => {
                todo!("tokio child stdout/stdin close")
            }
            #[cfg(feature = "char-device")]
            DuplexResources::CharDevice(char_device) => Pin::new(char_device).poll_shutdown(cx),
            #[cfg(feature = "socketpair")]
            DuplexResources::SocketpairStream(socketpair_stream) => {
                Pin::new(socketpair_stream).poll_shutdown(cx)
            }
            DuplexResources::PipeReaderWriter(_)
            | DuplexResources::SocketedThreadFunc(_)
            | DuplexResources::SocketedThread(_)
            | DuplexResources::SocketedThreadReadReady(_) => todo!("tokio duplex resources"),
        }
    }
}

impl Duplex for TokioStreamDuplexer {}

#[cfg(not(windows))]
impl AsRawFd for TokioStreamReader {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        match &self.resources {
            ReadResources::File(file) => file.as_raw_fd(),
            ReadResources::TcpStream(tcp_stream) => tcp_stream.as_raw_fd(),
            #[cfg(unix)]
            ReadResources::UnixStream(unix_stream) => unix_stream.as_raw_fd(),
            ReadResources::PipeReader(_pipe_reader) => todo!("tokio pipe as_raw_fd"),
            ReadResources::Stdin(_stdin) => todo!("tokio stdin as_raw_fd"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::PipedThread(_piped_thread) => todo!("tokio piped_thread as_raw_fd"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::Child(_child) => todo!("tokio child as_raw_fd"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::ChildStdout(_child_stdout) => todo!("tokio child stdout as_raw_fd"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::ChildStderr(_child_stderr) => todo!("tokio child stderr as_raw_fd"),
        }
    }
}

#[cfg(not(windows))]
impl AsRawFd for TokioStreamWriter {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        match &self.resources {
            WriteResources::File(file) => Pin::new(file).as_raw_fd(),
            WriteResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).as_raw_fd(),
            #[cfg(unix)]
            WriteResources::UnixStream(unix_stream) => Pin::new(unix_stream).as_raw_fd(),
            WriteResources::PipeWriter(_pipe_writer) => todo!("tokio pipe as_raw_fd"),
            WriteResources::Stdout(_stdout) => todo!("tokio stdout as_raw_fd"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::PipedThread(_piped_thread) => todo!("tokio piped_thread as_raw_fd"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::Child(_child) => todo!("tokio child as_raw_fd"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::ChildStdin(_child_stdin) => todo!("tokio child stdin as_raw_fd"),
        }
    }
}

#[cfg(not(windows))]
impl AsRawReadWriteFd for TokioStreamDuplexer {
    #[inline]
    fn as_raw_read_fd(&self) -> RawFd {
        match &self.resources {
            DuplexResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).as_raw_fd(),
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => Pin::new(unix_stream).as_raw_fd(),
            DuplexResources::StdinStdout(_stdin_stdout) => todo!("tokio stdout as_raw_read_fd"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("tokio child as_raw_read_fd"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_stdout_stdin) => {
                todo!("tokio child stdout/stdin as_raw_read_fd")
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
            | DuplexResources::SocketedThreadReadReady(_) => todo!("tokio duplex resources"),
        }
    }

    #[inline]
    fn as_raw_write_fd(&self) -> RawFd {
        match &self.resources {
            DuplexResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).as_raw_fd(),
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => Pin::new(unix_stream).as_raw_fd(),
            DuplexResources::StdinStdout(_stdin_stdout) => todo!("tokio stdout as_raw_write_fd"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("tokio child as_raw_write_fd"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_stdout_stdin) => {
                todo!("tokio child stdout/stdin as_raw_write_fd")
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
            | DuplexResources::SocketedThreadReadReady(_) => todo!("tokio duplex resources"),
        }
    }
}

#[cfg(windows)]
impl AsRawHandleOrSocket for TokioStreamReader {
    #[inline]
    fn as_raw_handle_or_socket(&self) -> RawHandleOrSocket {
        match &self.resources {
            ReadResources::File(file) => file.as_raw_handle_or_socket(),
            ReadResources::TcpStream(tcp_stream) => tcp_stream.as_raw_handle_or_socket(),
            #[cfg(unix)]
            ReadResources::UnixStream(unix_stream) => unix_stream.as_raw_handle_or_socket(),
            ReadResources::PipeReader(_pipe_reader) => todo!("tokio pipe as_raw_handle_or_socket"),
            ReadResources::Stdin(_stdin) => todo!("tokio stdin as_raw_handle_or_socket"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::PipedThread(_piped_thread) => {
                todo!("tokio piped_thread as_raw_handle_or_socket")
            }
            #[cfg(not(target_os = "wasi"))]
            ReadResources::Child(_child) => todo!("tokio child as_raw_handle_or_socket"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::ChildStdout(_child_stdout) => {
                todo!("tokio child stdout as_raw_handle_or_socket")
            }
            #[cfg(not(target_os = "wasi"))]
            ReadResources::ChildStderr(_child_stderr) => {
                todo!("tokio child stderr as_raw_handle_or_socket")
            }
        }
    }
}

#[cfg(windows)]
impl AsRawHandleOrSocket for TokioStreamWriter {
    #[inline]
    fn as_raw_handle_or_socket(&self) -> RawHandleOrSocket {
        match &self.resources {
            WriteResources::File(file) => Pin::new(file).as_raw_handle_or_socket(),
            WriteResources::TcpStream(tcp_stream) => Pin::new(tcp_stream).as_raw_handle_or_socket(),
            #[cfg(unix)]
            WriteResources::UnixStream(unix_stream) => {
                Pin::new(unix_stream).as_raw_handle_or_socket()
            }
            WriteResources::PipeWriter(_pipe_writer) => todo!("tokio pipe as_raw_handle_or_socket"),
            WriteResources::Stdout(_stdout) => todo!("tokio stdout as_raw_handle_or_socket"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::PipedThread(_piped_thread) => {
                todo!("tokio piped_thread as_raw_handle_or_socket")
            }
            #[cfg(not(target_os = "wasi"))]
            WriteResources::Child(_child) => todo!("tokio child as_raw_handle_or_socket"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::ChildStdin(_child_stdin) => {
                todo!("tokio child stdin as_raw_handle_or_socket")
            }
        }
    }
}

#[cfg(windows)]
impl AsRawReadWriteHandleOrSocket for TokioStreamDuplexer {
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
                todo!("tokio stdout as_raw_handle_or_socket")
            }
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("tokio child as_raw_handle_or_socket"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_stdout_stdin) => {
                todo!("tokio child stdout/stdin as_raw_handle_or_socket")
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
            | DuplexResources::SocketedThreadReadReady(_) => todo!("tokio duplex resources"),
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
                todo!("tokio stdout as_raw_handle_or_socket")
            }
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("tokio child as_raw_handle_or_socket"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_stdout_stdin) => {
                todo!("tokio child stdout/stdin as_raw_handle_or_socket")
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
            | DuplexResources::SocketedThreadReadReady(_) => todo!("tokio duplex resources"),
        }
    }
}

#[cfg(not(windows))]
impl AsFd for TokioStreamReader {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        match &self.resources {
            ReadResources::File(file) => file.as_fd(),
            ReadResources::TcpStream(tcp_stream) => tcp_stream.as_fd(),
            #[cfg(unix)]
            ReadResources::UnixStream(unix_stream) => unix_stream.as_fd(),
            ReadResources::PipeReader(_pipe_reader) => todo!("tokio pipe as_fd"),
            ReadResources::Stdin(_stdin) => todo!("tokio stdin as_fd"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::PipedThread(_piped_thread) => todo!("tokio piped_thread as_fd"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::Child(_child) => todo!("tokio child as_fd"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::ChildStdout(_child_stdout) => todo!("tokio child stdout as_fd"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::ChildStderr(_child_stderr) => todo!("tokio child stderr as_fd"),
        }
    }
}

#[cfg(not(windows))]
impl AsFd for TokioStreamWriter {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        match &self.resources {
            WriteResources::File(file) => file.as_fd(),
            WriteResources::TcpStream(tcp_stream) => tcp_stream.as_fd(),
            #[cfg(unix)]
            WriteResources::UnixStream(unix_stream) => unix_stream.as_fd(),
            WriteResources::PipeWriter(_pipe_writer) => todo!("tokio pipe as_fd"),
            WriteResources::Stdout(_stdout) => todo!("tokio stdout as_fd"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::PipedThread(_piped_thread) => todo!("tokio piped_thread as_fd"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::Child(_child) => todo!("tokio child as_fd"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::ChildStdin(_child_stdin) => todo!("tokio child stdin as_fd"),
        }
    }
}

#[cfg(not(windows))]
impl AsReadWriteFd for TokioStreamDuplexer {
    #[inline]
    fn as_read_fd(&self) -> BorrowedFd<'_> {
        match &self.resources {
            DuplexResources::TcpStream(tcp_stream) => tcp_stream.as_fd(),
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => unix_stream.as_fd(),
            DuplexResources::StdinStdout(_stdin_stdout) => todo!("tokio stdout as_read_fd"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("tokio child as_read_fd"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_stdout_stdin) => {
                todo!("tokio child stdout/stdin as_read_fd")
            }
            #[cfg(feature = "char-device")]
            DuplexResources::CharDevice(char_device) => char_device.as_fd(),
            #[cfg(feature = "socketpair")]
            DuplexResources::SocketpairStream(socketpair_stream) => socketpair_stream.as_fd(),
            DuplexResources::PipeReaderWriter(_)
            | DuplexResources::SocketedThreadFunc(_)
            | DuplexResources::SocketedThread(_)
            | DuplexResources::SocketedThreadReadReady(_) => todo!("tokio duplex resources"),
        }
    }

    #[inline]
    fn as_write_fd(&self) -> BorrowedFd<'_> {
        match &self.resources {
            DuplexResources::TcpStream(tcp_stream) => tcp_stream.as_fd(),
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => unix_stream.as_fd(),
            DuplexResources::StdinStdout(_stdin_stdout) => todo!("tokio stdout as_write_fd"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("tokio child as_write_fd"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_stdout_stdin) => {
                todo!("tokio child stdout/stdin as_write_fd")
            }
            #[cfg(feature = "char-device")]
            DuplexResources::CharDevice(char_device) => char_device.as_fd(),
            #[cfg(feature = "socketpair")]
            DuplexResources::SocketpairStream(socketpair_stream) => socketpair_stream.as_fd(),
            DuplexResources::PipeReaderWriter(_)
            | DuplexResources::SocketedThreadFunc(_)
            | DuplexResources::SocketedThread(_)
            | DuplexResources::SocketedThreadReadReady(_) => todo!("tokio duplex resources"),
        }
    }
}

#[cfg(windows)]
impl AsHandleOrSocket for TokioStreamReader {
    #[inline]
    fn as_handle_or_socket(&self) -> BorrowedHandleOrSocket<'_> {
        match &self.resources {
            ReadResources::File(file) => file.as_handle_or_socket(),
            ReadResources::TcpStream(tcp_stream) => tcp_stream.as_handle_or_socket(),
            #[cfg(unix)]
            ReadResources::UnixStream(unix_stream) => unix_stream.as_handle_or_socket(),
            ReadResources::PipeReader(_pipe_reader) => todo!("tokio pipe as_handle_or_socket"),
            ReadResources::Stdin(_stdin) => todo!("tokio stdin as_handle_or_socket"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::PipedThread(_piped_thread) => {
                todo!("tokio piped_thread as_handle_or_socket")
            }
            #[cfg(not(target_os = "wasi"))]
            ReadResources::Child(_child) => todo!("tokio child as_handle_or_socket"),
            #[cfg(not(target_os = "wasi"))]
            ReadResources::ChildStdout(_child_stdout) => {
                todo!("tokio child stdout as_handle_or_socket")
            }
            #[cfg(not(target_os = "wasi"))]
            ReadResources::ChildStderr(_child_stderr) => {
                todo!("tokio child stderr as_handle_or_socket")
            }
        }
    }
}

#[cfg(windows)]
impl AsHandleOrSocket for TokioStreamWriter {
    #[inline]
    fn as_handle_or_socket(&self) -> BorrowedHandleOrSocket<'_> {
        match &self.resources {
            WriteResources::File(file) => file.as_handle_or_socket(),
            WriteResources::TcpStream(tcp_stream) => tcp_stream.as_handle_or_socket(),
            #[cfg(unix)]
            WriteResources::UnixStream(unix_stream) => unix_stream.as_handle_or_socket(),
            WriteResources::PipeWriter(_pipe_writer) => todo!("tokio pipe as_handle_or_socket"),
            WriteResources::Stdout(_stdout) => todo!("tokio stdout as_handle_or_socket"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::PipedThread(_piped_thread) => {
                todo!("tokio piped_thread as_handle_or_socket")
            }
            #[cfg(not(target_os = "wasi"))]
            WriteResources::Child(_child) => todo!("tokio child as_handle_or_socket"),
            #[cfg(not(target_os = "wasi"))]
            WriteResources::ChildStdin(_child_stdin) => {
                todo!("tokio child stdin as_handle_or_socket")
            }
        }
    }
}

#[cfg(windows)]
impl AsReadWriteHandleOrSocket for TokioStreamDuplexer {
    #[inline]
    fn as_read_handle_or_socket(&self) -> BorrowedHandleOrSocket<'_> {
        match &self.resources {
            DuplexResources::TcpStream(tcp_stream) => tcp_stream.as_handle_or_socket(),
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => unix_stream.as_handle_or_socket(),
            DuplexResources::StdinStdout(_stdin_stdout) => {
                todo!("tokio stdout as_handle_or_socket")
            }
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("tokio child as_handle_or_socket"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_stdout_stdin) => {
                todo!("tokio child stdout/stdin as_handle_or_socket")
            }
            #[cfg(feature = "char-device")]
            DuplexResources::CharDevice(char_device) => char_device.as_handle_or_socket(),
            #[cfg(feature = "socketpair")]
            DuplexResources::SocketpairStream(socketpair_stream) => {
                socketpair_stream.as_handle_or_socket()
            }
            DuplexResources::PipeReaderWriter(_)
            | DuplexResources::SocketedThreadFunc(_)
            | DuplexResources::SocketedThread(_)
            | DuplexResources::SocketedThreadReadReady(_) => todo!("tokio duplex resources"),
        }
    }

    #[inline]
    fn as_write_handle_or_socket(&self) -> BorrowedHandleOrSocket<'_> {
        match &self.resources {
            DuplexResources::TcpStream(tcp_stream) => tcp_stream.as_handle_or_socket(),
            #[cfg(unix)]
            DuplexResources::UnixStream(unix_stream) => unix_stream.as_handle_or_socket(),
            DuplexResources::StdinStdout(_stdin_stdout) => {
                todo!("tokio stdout as_handle_or_socket")
            }
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::Child(_child) => todo!("tokio child as_handle_or_socket"),
            #[cfg(not(target_os = "wasi"))]
            DuplexResources::ChildStdoutStdin(_stdout_stdin) => {
                todo!("tokio child stdout/stdin as_handle_or_socket")
            }
            #[cfg(feature = "char-device")]
            DuplexResources::CharDevice(char_device) => char_device.as_handle_or_socket(),
            #[cfg(feature = "socketpair")]
            DuplexResources::SocketpairStream(socketpair_stream) => {
                socketpair_stream.as_handle_or_socket()
            }
            DuplexResources::PipeReaderWriter(_)
            | DuplexResources::SocketedThreadFunc(_)
            | DuplexResources::SocketedThread(_)
            | DuplexResources::SocketedThreadReadReady(_) => todo!("tokio duplex resources"),
        }
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

impl Debug for TokioStreamReader {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Just print the fd number; don't try to print the path or any
        // information about it, because this information is otherwise
        // unavailable to safe Rust code.
        f.debug_struct("TokioStreamReader")
            .field("raw_grip", &self.as_raw_grip())
            .finish()
    }
}

impl Debug for TokioStreamWriter {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Just print the fd number; don't try to print the path or any
        // information about it, because this information is otherwise
        // unavailable to safe Rust code.
        f.debug_struct("TokioStreamWriter")
            .field("raw_grip", &self.as_raw_grip())
            .finish()
    }
}

impl Debug for TokioStreamDuplexer {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Just print the fd numbers; don't try to print the path or any
        // information about it, because this information is otherwise
        // unavailable to safe Rust code.
        f.debug_struct("TokioStreamDuplexer")
            .field("raw_read_grip", &self.as_raw_read_grip())
            .field("raw_write_grip", &self.as_raw_write_grip())
            .finish()
    }
}

/// A trait that combines [`HalfDuplex`] and [`ReadReady`]. Implemented via
/// blanket implementation.
#[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
pub trait HalfDuplexReadReady: HalfDuplex + ReadReady {}

#[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
impl<T: HalfDuplex + ReadReady> HalfDuplexReadReady for T {}
