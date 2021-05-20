//! Unbuffered and unlocked I/O streams.
//!
//! For a starting point, see [`StreamReader`] and [`StreamWriter`] for input and
//! output streams. There's also [`StreamDuplexer`] for interactive streams.
//!
//! Since these types are unbuffered, it's advisable for most use cases to wrap
//! them in buffering types such as [`std::io::BufReader`], [`std::io::BufWriter`],
//! [`std::io::LineWriter`], [`BufDuplexer`], or [`BufReaderLineWriter`].
//!
//! [`BufReader`]: std::io::BufReader
//! [`BufWriter`]: std::io::BufWriter
//! [`LineWriter`]: std::io::LineWriter
//! [`AsRawFd`]: std::os::unix::io::AsRawFd
//! [pipe]: https://crates.io/crates/os_pipe

#![deny(missing_docs)]
#![cfg_attr(can_vector, feature(can_vector))]
#![cfg_attr(write_all_vectored, feature(write_all_vectored))]
#![cfg_attr(read_initializer, feature(read_initializer))]
#![cfg_attr(target_os = "wasi", feature(wasi_ext))]

#[cfg(feature = "async-std")]
mod async_std;
mod buffered;
mod lockers;
mod streams;
#[cfg(feature = "tokio")]
mod tokio;

#[cfg(feature = "async-std")]
pub use crate::async_std::{AsyncStreamDuplexer, AsyncStreamReader, AsyncStreamWriter};
#[cfg(feature = "tokio")]
pub use crate::tokio::{TokioStreamDuplexer, TokioStreamReader, TokioStreamWriter};
pub use buffered::{BufDuplexer, BufReaderLineWriter, IntoInnerError};
pub use streams::{StreamDuplexer, StreamReader, StreamWriter};
