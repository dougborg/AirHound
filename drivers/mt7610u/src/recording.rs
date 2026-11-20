//! Private, create-only recording files.
use anyhow::{Context, Result};
use std::{
    fs::{File, OpenOptions},
    io::BufWriter,
    path::PathBuf,
};
pub(crate) fn new_private_file(path: PathBuf) -> Result<BufWriter<File>> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(BufWriter::new(options.open(&path).with_context(|| {
        format!("creating {} (never overwrite)", path.display())
    })?))
}
