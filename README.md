<div align="center">
  <h1><code>io-streams</code></h1>

  <p>
    <strong>Unbuffered and unlocked I/O streams</strong>
  </p>

  <p>
    <a href="https://github.com/sunfishcode/io-streams/actions?query=workflow%3ACI"><img src="https://github.com/sunfishcode/io-streams/workflows/CI/badge.svg" alt="Github Actions CI Status" /></a>
    <a href="https://crates.io/crates/io-streams"><img src="https://img.shields.io/crates/v/io-streams.svg" alt="crates.io page" /></a>
    <a href="https://docs.rs/io-streams"><img src="https://docs.rs/io-streams/badge.svg" alt="docs.rs docs" /></a>
  </p>
</div>

This crate defines [`StreamReader`], [`StreamWriter`], and [`StreamDuplexer`]
types which provide safe, owning, unbuffered, and unlocked access to a raw I/O
stream, such as standard input, standard output, files, sockets, pipes, or
character devices. It also supports a "piped thread" concept, where an
arbitrary `Box<dyn Read + Send>` or `Box<dyn Write + Send>` can be provided,
and the I/O is performed on a thread and connecting to the `StreamReader` or
`StreamWriter` with a [pipe], and a "socketed thread" concept, where a provided
function is called on a thread and connected to the main thread via a
bidirectional socket.

This crate also defines [`AsyncStreamReader`], [`AsyncStreamWriter`], and
[`AsyncStreamDuplexer`], which are async functions that work with `async-std`.
And [`TokioStreamReader`], [`TokioStreamWriter`], and [`TokioStreamDuplexer`],
which are async functions that work with `tokio`. Not all features are
supported yet, and they aren't fully optimized yet, but basic file and socket
support is in place.

On Posix-ish platforms, including limited support for WASI, these types just
contain a single file descriptor (and implement [`AsFd`]), plus any
resources needed to safely hold the file descriptor live. On Windows, they
contain an enum holding either `RawHandle` or `RawSocket`.

Since these types are unbuffered, it's advisable for most use cases to wrap
them in buffering types such as [`std::io::BufReader`], [`std::io::BufWriter`],
[`std::io::LineWriter`], [`io_streams::BufDuplexer`], or
[`io_streams::BufReaderLineWriter`].

Rust's [`std::io::Stdin`] and [`std::io::Stdout`] are always buffered, while
its [`std::fs::File`] and [`std::net::TcpStream`] are unbuffered. A key purpose
of the `io_streams` crate is to abstract over the underlying inputs and outputs
without adding buffering, so that buffering can be applied without redundancy.

This crate locks `stdio::io::Stdin` and `std::io::Stdout` while it has their
corresponding streams open, to prevent accidental mixing of buffered and
unbuffered output on the same stream. Attempts to use the buffered streams when
they are locked will block indefinitely.

Support for async-std and tokio in char-device and socketpair is temporarily
disabled until those crates contain the needed implementations of the
I/O safety traits.

[`StreamReader`]: https://docs.rs/io-streams/latest/io_streams/struct.StreamReader.html
[`StreamWriter`]: https://docs.rs/io-streams/latest/io_streams/struct.StreamWriter.html
[`StreamDuplexer`]: https://docs.rs/io-streams/latest/io_streams/struct.StreamDuplexer.html
[`AsyncStreamReader`]: https://docs.rs/io-streams/latest/io_streams/struct.AsyncStreamReader.html
[`AsyncStreamWriter`]: https://docs.rs/io-streams/latest/io_streams/struct.AsyncStreamWriter.html
[`AsyncStreamDuplexer`]: https://docs.rs/io-streams/latest/io_streams/struct.AsyncStreamDuplexer.html
[`TokioStreamReader`]: https://docs.rs/io-streams/latest/io_streams/struct.TokioStreamReader.html
[`TokioStreamWriter`]: https://docs.rs/io-streams/latest/io_streams/struct.TokioStreamWriter.html
[`TokioStreamDuplexer`]: https://docs.rs/io-streams/latest/io_streams/struct.TokioStreamDuplexer.html
[`io_streams::BufDuplexer`]: https://docs.rs/io-streams/latest/io_streams/struct.BufDuplexer.html
[`io_streams::BufReaderLineWriter`]: https://docs.rs/io-streams/latest/io_streams/struct.BufReaderLineWriter.html
[`std::io::Stdin`]: https://doc.rust-lang.org/std/io/struct.Stdin.html
[`std::io::Stdout`]: https://doc.rust-lang.org/std/io/struct.Stdout.html
[`std::io::BufReader`]: https://doc.rust-lang.org/std/io/struct.BufReader.html
[`std::io::BufWriter`]: https://doc.rust-lang.org/std/io/struct.BufWriter.html
[`std::io::LineWriter`]: https://doc.rust-lang.org/std/io/struct.LineWriter.html
[`AsFd`]: https://docs.rs/io-lifetimes/0.2.0/io_lifetimes/trait.AsFd.html
[pipe]: https://crates.io/crates/os_pipe
[`std::fs::File`]: https://doc.rust-lang.org/std/fs/struct.File.html
[`std::net::TcpStream`]: https://doc.rust-lang.org/std/net/struct.TcpStream.html
