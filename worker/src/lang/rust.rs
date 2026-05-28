use std::rc::Rc;

use anyhow::{Context, Result};
use common::{ExecConfig, File};

use crate::os::{FdEntry, Fs, FsEntry, Pipe, ProcessHandle};
use crate::util::*;

pub async fn run(
    config: ExecConfig,
    files: Vec<File>,
    primary_file: String,
    stdin: Pipe,
    stdout: Pipe,
) -> Result<()> {
    send_fetching_compiler();
    let mut fs = get_fs("rust")
        .await
        .context("Failed to get Rust filesystem")?;

    send_compiling();
    for file in files {
        fs.add_file_with_path(
            format!("/workdir/{}", file.name).as_bytes(),
            Rc::new(file.content),
        );
    }
    let compiled_pipe = Pipe::new();
    fs.add_entry_with_path(b"__compiled", FsEntry::Pipe(compiled_pipe.clone()));
    let proc = ProcessHandle::builder()
        .name("rustc")
        .fs(fs)
        .stderr(FdEntry::WriteFn(Rc::new(move |buf: &[u8]| {
            send_compiler_message(buf);
            buf.len()
        })))
        .arg("--target=wasm32-wasip1-threads")
        .arg("--sysroot=/")
        .arg("-Zno-parallel-backend")
        .arg("-Zthreads=1")
        .arg("-Ccodegen-units=1")
        .arg("-Ctarget-feature=+atomics,+bulk-memory,+mutable-globals")
        .arg("--color=always")
        .arg(format!("workdir/{}", primary_file))
        .arg("-o__compiled")
        .spawn_with_path(b"bin/rustc");

    let status_code = proc.proc.wait().await;
    status_code.check_success()?;

    let mut compiled = Vec::new();
    compiled_pipe.close();
    loop {
        let mut buf = [0u8; 4096];
        let len = compiled_pipe.read(&mut buf).await;
        if len == 0 {
            break;
        }
        compiled.extend_from_slice(&buf[..len]);
    }

    send_running();
    let mut fs = Fs::new();
    fs.add_entry_with_path(b"input.txt", FsEntry::Pipe(stdin.clone()));
    fs.add_entry_with_path(b"output.txt", FsEntry::Pipe(stdout.clone()));
    let proc = ProcessHandle::builder()
        .name("solution")
        .fs(fs)
        .stdin(FdEntry::Pipe(stdin))
        .stdout(FdEntry::Pipe(stdout))
        .stderr(FdEntry::WriteFn(Rc::new(move |buf: &[u8]| {
            send_stderr(buf);
            buf.len()
        })))
        .mem_limit(config.mem_limit)
        .time_limit(config.time_limit)
        .spawn_with_code(&compiled);

    let status_code = proc.proc.wait().await;
    status_code.check_success().context("Execution failed")?;
    Ok(())
}

pub async fn run_ls(_stdin: Pipe, _stdout: Pipe, _stderr: Pipe) -> Result<()> {
    crate::send_msg(common::WorkerLSResponse::FetchingCompiler);
    let _fs = get_fs("rust")
        .await
        .context("Failed to get Rust filesystem")?;

    crate::send_msg(common::WorkerLSResponse::Started);
    Ok(())
}
