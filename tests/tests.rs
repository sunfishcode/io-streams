#![cfg_attr(can_vector, feature(can_vector))]
#![cfg_attr(write_all_vectored, feature(write_all_vectored))]

use io_streams::{StreamReader, StreamWriter};
use std::{
    fs::{remove_file, File},
    io::{copy, Read, Write},
};
use tempfile::{tempdir, TempDir};
#[cfg(all(not(target_os = "wasi"), feature = "socketpair"))]
use {
    io_streams::StreamInteractor,
    socketpair::SocketpairStream,
    std::{io, str},
};

#[allow(unused)]
fn tmpdir() -> TempDir {
    tempdir().expect("expected to be able to create a temporary directory")
}

#[test]
fn test_copy() -> anyhow::Result<()> {
    let dir = tmpdir();
    let in_txt = dir.path().join("in.txt");
    let out_txt = dir.path().join("out.txt");

    let mut in_file = File::create(&in_txt)?;
    write!(in_file, "Hello, world!")?;

    // Test regular file I/O.
    {
        let mut input = StreamReader::file(File::open(&in_txt)?);
        let mut output = StreamWriter::file(File::create(&out_txt)?);
        copy(&mut input, &mut output)?;
        output.flush()?;
        let mut s = String::new();
        File::open(&out_txt)?.read_to_string(&mut s)?;
        assert_eq!(s, "Hello, world!");
        remove_file(&out_txt)?;
    }

    // Test I/O through piped threads.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    {
        let mut input = StreamReader::piped_thread(Box::new(File::open(&in_txt)?))?;
        let mut output = StreamWriter::piped_thread(Box::new(File::create(&out_txt)?))?;
        copy(&mut input, &mut output)?;
        output.flush()?;
        let mut s = String::new();
        File::open(&out_txt)?.read_to_string(&mut s)?;
        assert_eq!(s, "Hello, world!");
        remove_file(&out_txt)?;
    }

    // Test regular file I/O through piped threads, not because this is
    // amazingly useful, but because these things should compose and we can.
    // This also tests that `StreamReader` and `StreamWriter`
    // implement `Send`.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    {
        let mut input =
            StreamReader::piped_thread(Box::new(StreamReader::file(File::open(&in_txt)?)))?;
        let mut output =
            StreamWriter::piped_thread(Box::new(StreamWriter::file(File::create(&out_txt)?)))?;
        copy(&mut input, &mut output)?;
        output.flush()?;
        let mut s = String::new();
        File::open(&out_txt)?.read_to_string(&mut s)?;
        assert_eq!(s, "Hello, world!");
        remove_file(&out_txt)?;
    }

    // They compose with themselves too.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    {
        let mut input = StreamReader::piped_thread(Box::new(StreamReader::piped_thread(
            Box::new(StreamReader::file(File::open(&in_txt)?)),
        )?))?;
        let mut output = StreamWriter::piped_thread(Box::new(StreamWriter::piped_thread(
            Box::new(StreamWriter::file(File::create(&out_txt)?)),
        )?))?;
        copy(&mut input, &mut output)?;
        output.flush()?;
        let mut s = String::new();
        File::open(&out_txt)?.read_to_string(&mut s)?;
        assert_eq!(s, "Hello, world!");
        remove_file(&out_txt)?;
    }

    // Test flushing between writes.
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    {
        let mut input = StreamReader::piped_thread(Box::new(StreamReader::piped_thread(
            Box::new(StreamReader::file(File::open(&in_txt)?)),
        )?))?;
        let mut output = StreamWriter::piped_thread(Box::new(StreamWriter::piped_thread(
            Box::new(StreamWriter::file(File::create(&out_txt)?)),
        )?))?;
        copy(&mut input, &mut output)?;
        output.flush()?;
        let mut s = String::new();
        File::open(&out_txt)?.read_to_string(&mut s)?;
        assert_eq!(s, "Hello, world!");
        input = StreamReader::piped_thread(Box::new(StreamReader::piped_thread(Box::new(
            StreamReader::file(File::open(&in_txt)?),
        ))?))?;
        copy(&mut input, &mut output)?;
        output.flush()?;
        s = String::new();
        File::open(&out_txt)?.read_to_string(&mut s)?;
        assert_eq!(s, "Hello, world!Hello, world!");
        input = StreamReader::piped_thread(Box::new(StreamReader::piped_thread(Box::new(
            StreamReader::file(File::open(&in_txt)?),
        ))?))?;
        copy(&mut input, &mut output)?;
        output.flush()?;
        s = String::new();
        File::open(&out_txt)?.read_to_string(&mut s)?;
        assert_eq!(s, "Hello, world!Hello, world!Hello, world!");
        remove_file(&out_txt)?;
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
fn test_socketpair() -> anyhow::Result<()> {
    let mut thread = StreamInteractor::socketed_thread(Box::new(
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
