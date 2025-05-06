use std::rc::Rc;

use anyhow::{Context, Result};

use crate::{
    os::{FileEntry, ProcessHandle},
    util::*,
};

pub async fn run(code: Vec<u8>, input: FileEntry) -> Result<()> {
    send_fetching_compiler();
    let mut fs = get_fs("python")
        .await
        .context("failed to get Python filesystem")?;
    fs.add_file(fs.root(), b"solution.py", Rc::new(code));

    send_running();
    let exe = fs
        .get_file(fs.get(fs.root(), b"bin/python3.12.wasm").unwrap())
        .unwrap();
    let proc = ProcessHandle::builder()
        .fs(fs)
        .stdin(input)
        .stdout(FileEntry::WriteFn(Rc::new(move |buf: &[u8]| {
            send_stdout(buf);
            buf.len()
        })))
        .stderr(FileEntry::WriteFn(Rc::new(move |buf: &[u8]| {
            send_stderr(buf);
            buf.len()
        })))
        .env(b"PYTHONHOME=/".to_vec())
        .spawn(
            &exe,
            vec![b"/bin/python3.12.wasm".to_vec(), b"/solution.py".to_vec()],
        );

    proc.proc.wait().await;
    Ok(())
}
