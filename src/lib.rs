//! Unbuffered and unlocked I/O streams.
//!
//! For a starting point, see [`StreamReader`] and [`StreamWriter`] for input and
//! output streams. There's also [`StreamInteractor`] for interactive streams.
//!
//! Since these types are unbuffered, it's advisable for most use cases to wrap
//! them in buffering types such as [`std::io::BufReader`], [`std::io::BufWriter`],
//! [`std::io::LineWriter`], [`BufInteractor`], or [`BufReaderLineWriter`].
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

mod buffered;
mod lockers;
mod streams;

pub use buffered::{BufInteractor, BufReaderLineWriter, IntoInnerError};
pub use streams::{StreamInteractor, StreamReader, StreamWriter};
