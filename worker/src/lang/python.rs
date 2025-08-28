use std::rc::Rc;

use anyhow::{Context, Result};

use crate::{
    os::{FdEntry, FsEntry, Pipe, ProcessHandle},
    util::*,
};

pub async fn run(code: Vec<u8>, stdin: Pipe, stdout: Pipe) -> Result<()> {
    send_fetching_compiler();
    let mut fs = get_fs("python")
        .await
        .context("Failed to get Python filesystem")?;

    send_compiling();
    send_running();
    fs.add_file_with_path(b"solution.py", Rc::new(code));
    fs.add_entry_with_path(b"input.txt", FsEntry::Pipe(stdin.clone()));
    fs.add_entry_with_path(b"output.txt", FsEntry::Pipe(stdout.clone()));
    let proc = ProcessHandle::builder()
        .fs(fs)
        .stdin(FdEntry::Pipe(stdin))
        .stdout(FdEntry::Pipe(stdout))
        .stderr(FdEntry::WriteFn(Rc::new(move |buf: &[u8]| {
            send_stderr(buf);
            buf.len()
        })))
        .env(b"PYTHONHOME=/".to_vec())
        .spawn_with_path(
            b"bin/python3.13.wasm",
            vec![b"/bin/python3.13.wasm".to_vec(), b"solution.py".to_vec()],
        );

    let status_code = proc.proc.wait().await;
    status_code.check_success().context("Execution failed")?;
    Ok(())
}
