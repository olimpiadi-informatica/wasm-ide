use std::rc::Rc;

use anyhow::{Context, Result};
use common::{ExecConfig, File};

use crate::os::{FdEntry, FsEntry, Pipe, ProcessHandle};
use crate::util::*;

pub async fn run(config: ExecConfig, files: Vec<File>, stdin: Pipe, stdout: Pipe) -> Result<()> {
    send_fetching_compiler();
    let mut fs = get_fs("python")
        .await
        .context("Failed to get Python filesystem")?;

    send_running();
    let main = files[0].name.clone();
    for file in files {
        fs.add_file_with_path(
            format!("/tmp/{}", file.name).as_bytes(),
            Rc::new(file.content.into_bytes()),
        );
    }
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
        .arg("/bin/python3.13.wasm")
        .arg(format!("/tmp/{main}"))
        .max_memory(config.mem_limit)
        .spawn_with_path(b"bin/python3.13.wasm");

    let status_code = proc.proc.wait().await;
    status_code.check_success().context("Execution failed")?;
    Ok(())
}

pub async fn run_ls(stdin: Pipe, stdout: Pipe, stderr: Pipe) -> Result<()> {
    crate::send_msg(common::WorkerLSResponse::FetchingCompiler);
    let mut fs = get_fs("python")
        .await
        .context("Failed to get Python filesystem")?;
    fs.add_file_with_path(b"/ruff.toml", Rc::new(b"indent-width = 2".to_vec()));
    let proc = ProcessHandle::builder()
        .fs(fs)
        .stdin(FdEntry::Pipe(stdin))
        .stdout(FdEntry::Pipe(stdout))
        .stderr(FdEntry::Pipe(stderr))
        .arg("ruff")
        .arg("server")
        .spawn_with_path(b"bin/ruff.wasm");

    crate::send_msg(common::WorkerLSResponse::Started);
    let status_code = proc.proc.wait().await;
    status_code.check_success().context("ruff failed")?;
    Ok(())
}
