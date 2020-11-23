use io_streams::{StreamReader, StreamWriter};
use std::io::copy;

fn main() -> anyhow::Result<()> {
    let mut input = StreamReader::stdin()?;
    let mut output = StreamWriter::stdout()?;
    copy(&mut input, &mut output)?;
    Ok(())
}
