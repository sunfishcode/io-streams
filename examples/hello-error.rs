use io_streams::{StreamReader, StreamWriter};
use std::io::copy;

fn main() -> anyhow::Result<()> {
    #[cfg(not(target_os = "wasi"))] // WASI doesn't support pipes yet
    let mut input = StreamReader::str("hello world\n")?;

    #[cfg(target_os = "wasi")]
    let mut input = StreamReader::stdin()?;
    assert!(!cfg!(target_os = "wasi"), "WASI doesn't support pipes yet");

    let mut output = StreamWriter::stderr()?;
    copy(&mut input, &mut output)?;
    Ok(())
}
