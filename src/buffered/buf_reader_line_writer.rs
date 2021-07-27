//! This file is derived from Rust's library/std/src/io/buffered at revision
//! f7801d6c7cc19ab22bdebcc8efa894a564c53469.

use super::buf_duplexer::BufDuplexerBackend;
use super::{BufReaderLineWriterShim, IntoInnerError};
use duplex::HalfDuplex;
#[cfg(feature = "layered-io")]
use layered_io::{Bufferable, HalfDuplexLayered};
use std::fmt;
#[cfg(read_initializer)]
use std::io::Initializer;
use std::io::{self, BufRead, IoSlice, IoSliceMut, Read, Write};
#[cfg(not(windows))]
use {
    io_lifetimes::{AsFd, BorrowedFd},
    unsafe_io::os::posish::{AsRawFd, RawFd},
};
#[cfg(windows)]
use {
    io_lifetimes::{AsHandle, AsSocket, BorrowedHandle, BorrowedSocket},
    std::os::windows::io::{AsRawHandle, AsRawSocket, RawHandle, RawSocket},
    unsafe_io::os::windows::{
        AsHandleOrSocket, AsRawHandleOrSocket, BorrowedHandleOrSocket, RawHandleOrSocket,
    },
};

/// Wraps a reader and writer and buffers input and output to and from it,
/// flushing the writer whenever a newline (`0x0a`, `'\n'`) is detected on
/// output.
///
/// The [`BufDuplexer`] struct wraps a reader and writer and buffers their
/// input and output. But it only does this batched write when it goes out of
/// scope, or when the internal buffer is full. Sometimes, you'd prefer to
/// write each line as it's completed, rather than the entire buffer at once.
/// Enter `BufReaderLineWriter`. It does exactly that.
///
/// Like [`BufDuplexer`], a `BufReaderLineWriter`’s buffer will also be flushed
/// when the `BufReaderLineWriter` goes out of scope or when its internal
/// buffer is full.
///
/// If there's still a partial line in the buffer when the
/// `BufReaderLineWriter` is dropped, it will flush those contents.
///
/// # Examples
///
/// We can use `BufReaderLineWriter` to write one line at a time, significantly
/// reducing the number of actual writes to the file.
///
/// ```no_run
/// use char_device::CharDevice;
/// use io_streams::BufReaderLineWriter;
/// use std::fs;
/// use std::io::prelude::*;
///
/// fn main() -> std::io::Result<()> {
///     let road_not_taken = b"I shall be telling this with a sigh
/// Somewhere ages and ages hence:
/// Two roads diverged in a wood, and I -
/// I took the one less traveled by,
/// And that has made all the difference.";
///
///     let file = CharDevice::open("/dev/tty")?;
///     let mut file = BufReaderLineWriter::new(file);
///
///     file.write_all(b"I shall be telling this with a sigh")?;
///
///     // No bytes are written until a newline is encountered (or
///     // the internal buffer is filled).
///     assert_eq!(fs::read_to_string("poem.txt")?, "");
///     file.write_all(b"\n")?;
///     assert_eq!(
///         fs::read_to_string("poem.txt")?,
///         "I shall be telling this with a sigh\n",
///     );
///
///     // Write the rest of the poem.
///     file.write_all(
///         b"Somewhere ages and ages hence:
/// Two roads diverged in a wood, and I -
/// I took the one less traveled by,
/// And that has made all the difference.",
///     )?;
///
///     // The last line of the poem doesn't end in a newline, so
///     // we have to flush or drop the `BufReaderLineWriter` to finish
///     // writing.
///     file.flush()?;
///
///     // Confirm the whole poem was written.
///     assert_eq!(fs::read("poem.txt")?, &road_not_taken[..]);
///     Ok(())
/// }
/// ```
///
/// [`BufDuplexer`]: crate::BufDuplexer
pub struct BufReaderLineWriter<Inner: HalfDuplex> {
    inner: BufReaderLineWriterBackend<Inner>,
}

/// The "backend" of `BufReaderLineWriter`, split off so that the public
/// `BufReaderLineWriter` functions can do extra flushing, and we can
/// use the private `BufReaderLineWriterBackend` functions internally
/// after flushing is already done.
struct BufReaderLineWriterBackend<Inner: HalfDuplex> {
    inner: BufDuplexerBackend<Inner>,
}

impl<Inner: HalfDuplex> BufReaderLineWriter<Inner> {
    /// Creates a new `BufReaderLineWriter`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use char_device::CharDevice;
    /// use io_streams::BufReaderLineWriter;
    ///
    /// fn main() -> std::io::Result<()> {
    ///     let file = CharDevice::open("/dev/tty")?;
    ///     let file = BufReaderLineWriter::new(file);
    ///     Ok(())
    /// }
    /// ```
    #[inline]
    pub fn new(inner: Inner) -> Self {
        Self {
            inner: BufReaderLineWriterBackend::new(inner),
        }
    }

    /// Creates a new `BufReaderLineWriter` with a specified capacities for the
    /// internal buffers.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use char_device::CharDevice;
    /// use io_streams::BufReaderLineWriter;
    ///
    /// fn main() -> std::io::Result<()> {
    ///     let file = CharDevice::open("/dev/tty")?;
    ///     let file = BufReaderLineWriter::with_capacities(10, 100, file);
    ///     Ok(())
    /// }
    /// ```
    #[inline]
    pub fn with_capacities(reader_capacity: usize, writer_capacity: usize, inner: Inner) -> Self {
        Self {
            inner: BufReaderLineWriterBackend::with_capacities(
                reader_capacity,
                writer_capacity,
                inner,
            ),
        }
    }

    /// Gets a reference to the underlying writer.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use char_device::CharDevice;
    /// use io_streams::BufReaderLineWriter;
    ///
    /// fn main() -> std::io::Result<()> {
    ///     let file = CharDevice::open("/dev/tty")?;
    ///     let file = BufReaderLineWriter::new(file);
    ///
    ///     let reference = file.get_ref();
    ///     Ok(())
    /// }
    /// ```
    #[inline]
    pub fn get_ref(&self) -> &Inner {
        self.inner.get_ref()
    }

    /// Gets a mutable reference to the underlying writer.
    ///
    /// Caution must be taken when calling methods on the mutable reference
    /// returned as extra writes could corrupt the output stream.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use char_device::CharDevice;
    /// use io_streams::BufReaderLineWriter;
    ///
    /// fn main() -> std::io::Result<()> {
    ///     let file = CharDevice::open("/dev/tty")?;
    ///     let mut file = BufReaderLineWriter::new(file);
    ///
    ///     // we can use reference just like file
    ///     let reference = file.get_mut();
    ///     Ok(())
    /// }
    /// ```
    #[inline]
    pub fn get_mut(&mut self) -> &mut Inner {
        self.inner.get_mut()
    }

    /// Unwraps this `BufReaderLineWriter`, returning the underlying writer.
    ///
    /// The internal buffer is written out before returning the writer.
    ///
    /// # Errors
    ///
    /// An [`Err`] will be returned if an error occurs while flushing the
    /// buffer.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use char_device::CharDevice;
    /// use io_streams::BufReaderLineWriter;
    ///
    /// fn main() -> std::io::Result<()> {
    ///     let file = CharDevice::open("/dev/tty")?;
    ///
    ///     let writer: BufReaderLineWriter<CharDevice> = BufReaderLineWriter::new(file);
    ///
    ///     let file: CharDevice = writer.into_inner()?;
    ///     Ok(())
    /// }
    /// ```
    #[inline]
    pub fn into_inner(self) -> Result<Inner, IntoInnerError<Self>> {
        self.inner
            .into_inner()
            .map_err(|err| err.new_wrapped(|inner| Self { inner }))
    }
}

impl<Inner: HalfDuplex> BufReaderLineWriterBackend<Inner> {
    pub fn new(inner: Inner) -> Self {
        // Lines typically aren't that long, don't use giant buffers
        Self::with_capacities(1024, 1024, inner)
    }

    pub fn with_capacities(reader_capacity: usize, writer_capacity: usize, inner: Inner) -> Self {
        Self {
            inner: BufDuplexerBackend::with_capacities(reader_capacity, writer_capacity, inner),
        }
    }

    #[inline]
    pub fn get_ref(&self) -> &Inner {
        self.inner.get_ref()
    }

    #[inline]
    pub fn get_mut(&mut self) -> &mut Inner {
        self.inner.get_mut()
    }

    pub fn into_inner(self) -> Result<Inner, IntoInnerError<Self>> {
        self.inner
            .into_inner()
            .map_err(|err| err.new_wrapped(|inner| Self { inner }))
    }
}

impl<Inner: HalfDuplex> Write for BufReaderLineWriter<Inner> {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.inner.write_vectored(bufs)
    }

    #[cfg(can_vector)]
    #[inline]
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.inner.write_all(buf)
    }

    #[cfg(write_all_vectored)]
    #[inline]
    fn write_all_vectored(&mut self, bufs: &mut [IoSlice<'_>]) -> io::Result<()> {
        self.inner.write_all_vectored(bufs)
    }

    #[inline]
    fn write_fmt(&mut self, fmt: fmt::Arguments<'_>) -> io::Result<()> {
        self.inner.write_fmt(fmt)
    }
}

impl<Inner: HalfDuplex> Write for BufReaderLineWriterBackend<Inner> {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        BufReaderLineWriterShim::new(&mut self.inner).write(buf)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        BufReaderLineWriterShim::new(&mut self.inner).write_vectored(bufs)
    }

    #[cfg(can_vector)]
    #[inline]
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        BufReaderLineWriterShim::new(&mut self.inner).write_all(buf)
    }

    #[cfg(write_all_vectored)]
    #[inline]
    fn write_all_vectored(&mut self, bufs: &mut [IoSlice<'_>]) -> io::Result<()> {
        BufReaderLineWriterShim::new(&mut self.inner).write_all_vectored(bufs)
    }

    #[inline]
    fn write_fmt(&mut self, fmt: fmt::Arguments<'_>) -> io::Result<()> {
        BufReaderLineWriterShim::new(&mut self.inner).write_fmt(fmt)
    }
}

impl<Inner: HalfDuplex> Read for BufReaderLineWriter<Inner> {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Flush the output buffer before reading.
        self.inner.flush()?;

        self.inner.read(buf)
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        // Flush the output buffer before reading.
        self.inner.flush()?;

        self.inner.read_vectored(bufs)
    }

    #[cfg(can_vector)]
    #[inline]
    fn is_read_vectored(&self) -> bool {
        self.inner.is_read_vectored()
    }

    #[cfg(read_initializer)]
    #[inline]
    unsafe fn initializer(&self) -> Initializer {
        self.inner.initializer()
    }
}

impl<Inner: HalfDuplex> Read for BufReaderLineWriterBackend<Inner> {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        self.inner.read_vectored(bufs)
    }

    #[cfg(can_vector)]
    #[inline]
    fn is_read_vectored(&self) -> bool {
        self.inner.is_read_vectored()
    }

    // we can't skip unconditionally because of the large buffer case in read.
    #[cfg(read_initializer)]
    #[inline]
    unsafe fn initializer(&self) -> Initializer {
        self.inner.initializer()
    }
}

impl<Inner: HalfDuplex> BufRead for BufReaderLineWriter<Inner> {
    #[inline]
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.inner.fill_buf()
    }

    #[inline]
    fn consume(&mut self, amt: usize) {
        self.inner.consume(amt)
    }

    #[inline]
    fn read_until(&mut self, byte: u8, buf: &mut Vec<u8>) -> io::Result<usize> {
        // Flush the output buffer before reading.
        self.flush()?;

        self.inner.read_until(byte, buf)
    }

    #[inline]
    fn read_line(&mut self, buf: &mut String) -> io::Result<usize> {
        // Flush the output buffer before reading.
        self.flush()?;

        let t = self.inner.read_line(buf)?;

        Ok(t)
    }
}

impl<Inner: HalfDuplex> BufRead for BufReaderLineWriterBackend<Inner> {
    #[inline]
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.inner.fill_buf()
    }

    #[inline]
    fn consume(&mut self, amt: usize) {
        self.inner.consume(amt)
    }
}

impl<Inner: HalfDuplex> fmt::Debug for BufReaderLineWriter<Inner>
where
    Inner: fmt::Debug,
{
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(fmt)
    }
}

impl<Inner: HalfDuplex> fmt::Debug for BufReaderLineWriterBackend<Inner>
where
    Inner: fmt::Debug,
{
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("BufReaderLineWriter")
            .field("inner", &self.get_ref())
            .field(
                "reader_buffer",
                &format_args!(
                    "{}/{}",
                    self.inner.reader_buffer().len(),
                    self.inner.reader_capacity()
                ),
            )
            .field(
                "writer_buffer",
                &format_args!(
                    "{}/{}",
                    self.inner.writer_buffer().len(),
                    self.inner.writer_capacity()
                ),
            )
            .finish()
    }
}

#[cfg(not(windows))]
impl<Inner: HalfDuplex + AsRawFd> AsRawFd for BufReaderLineWriter<Inner> {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

#[cfg(windows)]
impl<Inner: HalfDuplex + AsRawHandle> AsRawHandle for BufReaderLineWriter<Inner> {
    #[inline]
    fn as_raw_handle(&self) -> RawHandle {
        self.inner.as_raw_handle()
    }
}

#[cfg(windows)]
impl<Inner: HalfDuplex + AsRawSocket> AsRawSocket for BufReaderLineWriter<Inner> {
    #[inline]
    fn as_raw_socket(&self) -> RawSocket {
        self.inner.as_raw_socket()
    }
}

#[cfg(windows)]
impl<Inner: HalfDuplex + AsRawHandleOrSocket> AsRawHandleOrSocket for BufReaderLineWriter<Inner> {
    #[inline]
    fn as_raw_handle_or_socket(&self) -> RawHandleOrSocket {
        self.inner.as_raw_handle_or_socket()
    }
}

#[cfg(not(windows))]
impl<Inner: HalfDuplex + AsRawFd> AsRawFd for BufReaderLineWriterBackend<Inner> {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

#[cfg(windows)]
impl<Inner: HalfDuplex + AsRawHandle> AsRawHandle for BufReaderLineWriterBackend<Inner> {
    #[inline]
    fn as_raw_handle(&self) -> RawHandle {
        self.inner.as_raw_handle()
    }
}

#[cfg(windows)]
impl<Inner: HalfDuplex + AsRawSocket> AsRawSocket for BufReaderLineWriterBackend<Inner> {
    #[inline]
    fn as_raw_socket(&self) -> RawSocket {
        self.inner.as_raw_socket()
    }
}

#[cfg(windows)]
impl<Inner: HalfDuplex + AsRawHandleOrSocket> AsRawHandleOrSocket
    for BufReaderLineWriterBackend<Inner>
{
    #[inline]
    fn as_raw_handle_or_socket(&self) -> RawHandleOrSocket {
        self.inner.as_raw_handle_or_socket()
    }
}

#[cfg(not(windows))]
impl<Inner: HalfDuplex + AsFd> AsFd for BufReaderLineWriter<Inner> {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.as_fd()
    }
}

#[cfg(windows)]
impl<Inner: HalfDuplex + AsHandle> AsHandle for BufReaderLineWriter<Inner> {
    #[inline]
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.inner.as_handle()
    }
}

#[cfg(windows)]
impl<Inner: HalfDuplex + AsSocket> AsSocket for BufReaderLineWriter<Inner> {
    #[inline]
    fn as_socket(&self) -> BorrowedSocket<'_> {
        self.inner.as_socket()
    }
}

#[cfg(windows)]
impl<Inner: HalfDuplex + AsHandleOrSocket> AsHandleOrSocket for BufReaderLineWriter<Inner> {
    #[inline]
    fn as_handle_or_socket(&self) -> BorrowedHandleOrSocket<'_> {
        self.inner.as_handle_or_socket()
    }
}

#[cfg(not(windows))]
impl<Inner: HalfDuplex + AsFd> AsFd for BufReaderLineWriterBackend<Inner> {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.as_fd()
    }
}

#[cfg(windows)]
impl<Inner: HalfDuplex + AsHandle> AsHandle for BufReaderLineWriterBackend<Inner> {
    #[inline]
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.inner.as_handle()
    }
}

#[cfg(windows)]
impl<Inner: HalfDuplex + AsSocket> AsSocket for BufReaderLineWriterBackend<Inner> {
    #[inline]
    fn as_socket(&self) -> BorrowedSocket<'_> {
        self.inner.as_socket()
    }
}

#[cfg(windows)]
impl<Inner: HalfDuplex + AsHandleOrSocket> AsHandleOrSocket for BufReaderLineWriterBackend<Inner> {
    #[inline]
    fn as_handle_or_socket(&self) -> BorrowedHandleOrSocket<'_> {
        self.inner.as_handle_or_socket()
    }
}

#[cfg(feature = "terminal-io")]
impl<Inner: HalfDuplex + terminal_io::Terminal> terminal_io::Terminal
    for BufReaderLineWriter<Inner>
{
}

#[cfg(feature = "terminal-io")]
impl<Inner: HalfDuplex + terminal_io::Terminal> terminal_io::Terminal
    for BufReaderLineWriterBackend<Inner>
{
}

#[cfg(feature = "terminal-io")]
impl<Inner: HalfDuplex + terminal_io::WriteTerminal> terminal_io::WriteTerminal
    for BufReaderLineWriter<Inner>
{
    #[inline]
    fn color_support(&self) -> terminal_io::TerminalColorSupport {
        self.inner.color_support()
    }

    #[inline]
    fn color_preference(&self) -> bool {
        self.inner.color_preference()
    }

    #[inline]
    fn is_output_terminal(&self) -> bool {
        self.inner.is_output_terminal()
    }
}

#[cfg(feature = "terminal-io")]
impl<Inner: HalfDuplex + terminal_io::WriteTerminal> terminal_io::WriteTerminal
    for BufReaderLineWriterBackend<Inner>
{
    #[inline]
    fn color_support(&self) -> terminal_io::TerminalColorSupport {
        self.inner.color_support()
    }

    #[inline]
    fn color_preference(&self) -> bool {
        self.inner.color_preference()
    }

    #[inline]
    fn is_output_terminal(&self) -> bool {
        self.inner.is_output_terminal()
    }
}

#[cfg(feature = "layered-io")]
impl<Inner: HalfDuplexLayered> Bufferable for BufReaderLineWriter<Inner> {
    #[inline]
    fn abandon(&mut self) {
        self.inner.abandon()
    }

    #[inline]
    fn suggested_buffer_size(&self) -> usize {
        self.inner.suggested_buffer_size()
    }
}

#[cfg(feature = "layered-io")]
impl<Inner: HalfDuplexLayered> Bufferable for BufReaderLineWriterBackend<Inner> {
    #[inline]
    fn abandon(&mut self) {
        self.inner.abandon()
    }

    #[inline]
    fn suggested_buffer_size(&self) -> usize {
        self.inner.suggested_buffer_size()
    }
}
