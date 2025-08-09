use std::rc::Rc;

use anyhow::{Context, Result};

use crate::{
    os::{FdEntry, ProcessHandle},
    util::*,
};

pub async fn run(code: Vec<u8>, input: FdEntry) -> Result<()> {
    send_fetching_compiler();
    let mut fs = get_fs("python")
        .await
        .context("Failed to get Python filesystem")?;
    fs.add_file(fs.root(), b"solution.py", Rc::new(code));

    send_running();
    let exe = fs
        .get_file(fs.get(fs.root(), b"bin/python3.12.wasm").unwrap())
        .unwrap();
    let proc = ProcessHandle::builder()
        .fs(fs)
        .stdin(input)
        .stdout(FdEntry::WriteFn(Rc::new(move |buf: &[u8]| {
            send_stdout(buf);
            buf.len()
        })))
        .stderr(FdEntry::WriteFn(Rc::new(move |buf: &[u8]| {
            send_stderr(buf);
            buf.len()
        })))
        .env(b"PYTHONHOME=/".to_vec())
        .spawn(
            &exe,
            vec![b"/bin/python3.12.wasm".to_vec(), b"/solution.py".to_vec()],
        );

    let status_code = proc.proc.wait().await;
    status_code.check_success()?;
    Ok(())
}
