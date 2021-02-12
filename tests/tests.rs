#![cfg_attr(can_vector, feature(can_vector))]
#![cfg_attr(write_all_vectored, feature(write_all_vectored))]

use cap_tempfile::{tempdir, TempDir};
use io_streams::{StreamReader, StreamWriter};
use std::io::{self, copy, Read, Write};
#[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
use {io_streams::StreamDuplexer, socketpair::SocketpairStream, std::str};

#[allow(unused)]
fn tmpdir() -> TempDir {
    unsafe { tempdir() }.expect("expected to be able to create a temporary directory")
}

#[test]
fn test_copy() -> anyhow::Result<()> {
    let dir = tmpdir();
    let in_txt = "in.txt";
    let out_txt = "out.txt";

    let mut in_file = dir.create(in_txt)?;
    write!(in_file, "Hello, world!")?;

    // Test regular file I/O.
    {
        let mut input = StreamReader::file(dir.open(in_txt)?);
        let mut output = StreamWriter::file(dir.create(out_txt)?);
        copy(&mut input, &mut output)?;
        output.flush()?;
        let mut s = String::new();
        dir.open(out_txt)?.read_to_string(&mut s)?;
        assert_eq!(s, "Hello, world!");
        dir.remove_file(out_txt)?;
    }

    // Test I/O through piped threads.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    {
        let mut input = StreamReader::piped_thread(Box::new(dir.open(in_txt)?))?;
        let mut output = StreamWriter::piped_thread(Box::new(dir.create(out_txt)?))?;
        copy(&mut input, &mut output)?;
        output.flush()?;
        let mut s = String::new();
        dir.open(out_txt)?.read_to_string(&mut s)?;
        assert_eq!(s, "Hello, world!");
        dir.remove_file(out_txt)?;
    }

    // Test regular file I/O through piped threads, not because this is
    // amazingly useful, but because these things should compose and we can.
    // This also tests that `StreamReader` and `StreamWriter`
    // implement `Send`.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    {
        let mut input =
            StreamReader::piped_thread(Box::new(StreamReader::file(dir.open(in_txt)?)))?;
        let mut output =
            StreamWriter::piped_thread(Box::new(StreamWriter::file(dir.create(out_txt)?)))?;
        copy(&mut input, &mut output)?;
        output.flush()?;
        let mut s = String::new();
        dir.open(out_txt)?.read_to_string(&mut s)?;
        assert_eq!(s, "Hello, world!");
        dir.remove_file(out_txt)?;
    }

    // They compose with themselves too.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    {
        let mut input = StreamReader::piped_thread(Box::new(StreamReader::piped_thread(
            Box::new(StreamReader::file(dir.open(in_txt)?)),
        )?))?;
        let mut output = StreamWriter::piped_thread(Box::new(StreamWriter::piped_thread(
            Box::new(StreamWriter::file(dir.create(out_txt)?)),
        )?))?;
        copy(&mut input, &mut output)?;
        output.flush()?;
        let mut s = String::new();
        dir.open(out_txt)?.read_to_string(&mut s)?;
        assert_eq!(s, "Hello, world!");
        dir.remove_file(out_txt)?;
    }

    // Test flushing between writes.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    {
        let mut input = StreamReader::piped_thread(Box::new(StreamReader::piped_thread(
            Box::new(StreamReader::file(dir.open(in_txt)?)),
        )?))?;
        let mut output = StreamWriter::piped_thread(Box::new(StreamWriter::piped_thread(
            Box::new(StreamWriter::file(dir.create(out_txt)?)),
        )?))?;
        copy(&mut input, &mut output)?;
        output.flush()?;
        let mut s = String::new();
        dir.open(out_txt)?.read_to_string(&mut s)?;
        assert_eq!(s, "Hello, world!");
        input = StreamReader::piped_thread(Box::new(StreamReader::piped_thread(Box::new(
            StreamReader::file(dir.open(in_txt)?),
        ))?))?;
        copy(&mut input, &mut output)?;
        output.flush()?;
        s = String::new();
        dir.open(out_txt)?.read_to_string(&mut s)?;
        assert_eq!(s, "Hello, world!Hello, world!");
        input = StreamReader::piped_thread(Box::new(StreamReader::piped_thread(Box::new(
            StreamReader::file(dir.open(in_txt)?),
        ))?))?;
        copy(&mut input, &mut output)?;
        output.flush()?;
        s = String::new();
        dir.open(out_txt)?.read_to_string(&mut s)?;
        assert_eq!(s, "Hello, world!Hello, world!Hello, world!");
        dir.remove_file(out_txt)?;
    }

    Ok(())
}

#[test]
#[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
fn test_null() -> anyhow::Result<()> {
    let mut input = StreamReader::str("send to null")?;
    let mut output = StreamWriter::null()?;
    copy(&mut input, &mut output)?;
    output.flush()?;
    Ok(())
}

#[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
#[test]
fn test_socketed_thread_func() -> anyhow::Result<()> {
    let mut thread = StreamDuplexer::socketed_thread_func(Box::new(
        |mut stream: SocketpairStream| -> io::Result<SocketpairStream> {
            let mut buf = [0_u8; 4096];
            let n = stream.read(&mut buf)?;
            assert_eq!(str::from_utf8(&buf[..n]).unwrap(), "hello world\n");

            writeln!(stream, "greetings")?;
            stream.flush()?;

            let n = stream.read(&mut buf)?;
            assert_eq!(str::from_utf8(&buf[..n]).unwrap(), "goodbye\n");
            Ok(stream)
        },
    ))?;

    writeln!(thread, "hello world")?;
    thread.flush()?;

    let mut buf = [0_u8; 4096];
    let n = thread.read(&mut buf)?;
    assert_eq!(str::from_utf8(&buf[..n]).unwrap(), "greetings\n");

    writeln!(thread, "goodbye")?;
    thread.flush()?;

    Ok(())
}

struct Mock(bool);

impl Read for Mock {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        assert!(!self.0);
        self.0 = true;
        assert!(!buf.is_empty());
        let len = buf.len() - 1;
        buf[..len].copy_from_slice(&vec![0xab_u8; len]);
        Ok(len)
    }
}

impl Write for Mock {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        assert!(self.0);
        self.0 = false;
        assert_eq!(buf, &vec![0xcd_u8; buf.len()]);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl system_interface::io::ReadReady for Mock {
    fn num_ready_bytes(&self) -> io::Result<u64> {
        if self.0 {
            Ok(0)
        } else {
            Ok(1)
        }
    }
}

impl duplex::Duplex for Mock {}

#[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
#[test]
fn test_socketed_thread_read_first() -> anyhow::Result<()> {
    let mut thread = StreamDuplexer::socketed_thread_read_first(Box::new(Mock(false)))?;

    let mut buf = [0_u8; 4];

    let n = thread.read(&mut buf)?;
    assert_eq!(n, 4);
    assert_eq!(&buf, &[0xab_u8; 4]);

    let n = thread.write(b"\xcd\xcd\xcd\xcd")?;
    assert_eq!(n, 4);
    thread.flush()?;

    let n = thread.read(&mut buf)?;
    assert_eq!(n, 4);
    assert_eq!(&buf, &[0xab_u8; 4]);

    let n = thread.write(b"\xcd\xcd\xcd\xcd")?;
    assert_eq!(n, 4);
    thread.flush()?;

    Ok(())
}

#[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
#[test]
fn test_socketed_thread_write_first() -> anyhow::Result<()> {
    let mut thread = StreamDuplexer::socketed_thread_write_first(Box::new(Mock(true)))?;
    let mut buf = [0_u8; 4];

    let n = thread.write(b"\xcd\xcd\xcd\xcd")?;
    assert_eq!(n, 4);
    thread.flush()?;

    let n = thread.read(&mut buf)?;
    assert_eq!(n, 4);
    assert_eq!(&buf, &[0xab_u8; 4]);

    let n = thread.write(b"\xcd\xcd\xcd\xcd")?;
    assert_eq!(n, 4);
    thread.flush()?;

    let n = thread.read(&mut buf)?;
    assert_eq!(n, 4);
    assert_eq!(&buf, &[0xab_u8; 4]);

    Ok(())
}

#[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
#[test]
fn test_socketed_thread_auto_read_first() -> anyhow::Result<()> {
    let mut thread = StreamDuplexer::socketed_thread(Box::new(Mock(false)))?;

    let mut buf = [0_u8; 4];

    let n = thread.read(&mut buf)?;
    assert_eq!(n, 4);
    assert_eq!(&buf, &[0xab_u8; 4]);

    let n = thread.write(b"\xcd\xcd\xcd\xcd")?;
    assert_eq!(n, 4);
    thread.flush()?;

    let n = thread.read(&mut buf)?;
    assert_eq!(n, 4);
    assert_eq!(&buf, &[0xab_u8; 4]);

    let n = thread.write(b"\xcd\xcd\xcd")?;
    assert_eq!(n, 3);
    thread.flush()?;

    Ok(())
}

#[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
#[test]
fn test_socketed_thread_auto_write_first() -> anyhow::Result<()> {
    let mut thread = StreamDuplexer::socketed_thread(Box::new(Mock(true)))?;
    let mut buf = [0_u8; 4];

    let n = thread.write(b"\xcd\xcd\xcd\xcd")?;
    assert_eq!(n, 4);
    thread.flush()?;

    let n = thread.read(&mut buf)?;
    assert_eq!(n, 4);
    assert_eq!(&buf, &[0xab_u8; 4]);

    let n = thread.write(b"\xcd\xcd\xcd")?;
    assert_eq!(n, 3);
    thread.flush()?;

    let n = thread.read(&mut buf)?;
    assert_eq!(n, 4);
    assert_eq!(&buf, &[0xab_u8; 4]);

    Ok(())
}
