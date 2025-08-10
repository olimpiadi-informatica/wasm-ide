use std::{cell::RefCell, rc::Rc};

use anyhow::{Context, Result};

use crate::{
    os::{FdEntry, Fs, Pipe, ProcessHandle},
    util::*,
};

async fn compile(cpp: bool, fs: Fs, code: Vec<u8>) -> Result<Vec<u8>> {
    let lang = match cpp {
        true => &b"c++"[..],
        false => &b"c"[..],
    };
    let std = match cpp {
        true => &b"-std=c++20"[..],
        false => &b"-std=c17"[..],
    };
    let exe = fs
        .get_file_with_path(b"/bin/llvm")
        .context("Failed to get clang executable")?;
    let compiled = Rc::new(RefCell::new(Vec::new()));
    let compiled2 = compiled.clone();
    let proc = ProcessHandle::builder()
        .fs(fs.clone())
        .stdin(FdEntry::Data {
            data: code,
            offset: 0,
        })
        .stdout(FdEntry::WriteFn(Rc::new(move |buf: &[u8]| {
            compiled2.borrow_mut().extend_from_slice(buf);
            buf.len()
        })))
        .stderr(FdEntry::WriteFn(Rc::new(move |buf: &[u8]| {
            send_compiler_message(buf);
            buf.len()
        })))
        .spawn(
            &exe,
            vec![
                b"clang++".to_vec(),
                b"-cc1".to_vec(),
                b"-isysroot".to_vec(),
                b"/".to_vec(),
                b"-I/include/c++/15.0.0/wasm32-wasip1/".to_vec(),
                b"-I/include/c++/15.0.0/".to_vec(),
                b"-stdlib=libstdc++".to_vec(),
                b"-internal-isystem".to_vec(),
                b"/lib/clang/20/include".to_vec(),
                b"-internal-isystem".to_vec(),
                b"/include/wasm32-wasip1-threads".to_vec(),
                b"-I/include/".to_vec(),
                b"-resource-dir".to_vec(),
                b"lib/clang/20".to_vec(),
                b"-target-feature".to_vec(),
                b"+atomics".to_vec(),
                b"-target-feature".to_vec(),
                b"+bulk-memory".to_vec(),
                b"-target-feature".to_vec(),
                b"+mutable-globals".to_vec(),
                b"-I.".to_vec(),
                b"-x".to_vec(),
                lang.to_vec(),
                b"-O2".to_vec(),
                b"-Wall".to_vec(),
                std.to_vec(),
                b"-emit-obj".to_vec(),
                b"-".to_vec(),
                b"-o".to_vec(),
                b"-".to_vec(),
            ],
        );

    let status_code = proc.proc.wait().await;
    status_code.check_success()?;
    let compiled = std::mem::take(&mut *compiled.borrow_mut());
    Ok(compiled)
}

async fn link(mut fs: Fs, compiled: Vec<u8>) -> Result<Vec<u8>> {
    let exe = fs
        .get_file_with_path(b"/bin/llvm")
        .context("Failed to get wasm-ld executable")?;
    let linked = Rc::new(RefCell::new(Vec::new()));
    let linked2 = linked.clone();
    fs.add_file_with_path(b"source.o", Rc::new(compiled));
    let proc = ProcessHandle::builder()
        .fs(fs.clone())
        .stdout(FdEntry::WriteFn(Rc::new(move |buf: &[u8]| {
            linked2.borrow_mut().extend_from_slice(buf);
            buf.len()
        })))
        .stderr(FdEntry::WriteFn(Rc::new(move |buf: &[u8]| {
            send_compiler_message(buf);
            buf.len()
        })))
        .spawn(
            &exe,
            vec![
                b"wasm-ld".to_vec(),
                b"-L/lib/wasm32-wasip1-threads/".to_vec(),
                b"-lc".to_vec(),
                b"/lib/clang/20/lib/wasm32-unknown-wasip1-threads/libclang_rt.builtins.a".to_vec(),
                b"/lib/wasm32-wasip1-threads/crt1.o".to_vec(),
                b"-L/lib".to_vec(),
                b"-lstdc++".to_vec(),
                b"-lsupc++".to_vec(),
                b"-z".to_vec(),
                b"stack-size=16777216".to_vec(),
                b"--stack-first".to_vec(),
                b"--shared-memory".to_vec(),
                b"--import-memory".to_vec(),
                b"--export-memory".to_vec(),
                b"--max-memory=4294967296".to_vec(),
                b"-o".to_vec(),
                b"-".to_vec(),
                b"source.o".to_vec(),
            ],
        );

    let status_code = proc.proc.wait().await;
    status_code.check_success()?;
    let linked = std::mem::take(&mut *linked.borrow_mut());
    Ok(linked)
}

pub async fn run(cpp: bool, code: Vec<u8>, input: FdEntry) -> Result<()> {
    send_fetching_compiler();
    let fs = get_fs("cpp")
        .await
        .context("Failed to get C/C++ filesystem")?;

    send_compiling();
    let compiled = compile(cpp, fs.clone(), code)
        .await
        .context("Compilation failed")?;
    let linked = link(fs, compiled).await.context("Linking failed")?;

    send_running();
    let proc = ProcessHandle::builder()
        .stdin(input)
        .stdout(FdEntry::WriteFn(Rc::new(move |buf: &[u8]| {
            send_stdout(buf);
            buf.len()
        })))
        .stderr(FdEntry::WriteFn(Rc::new(move |buf: &[u8]| {
            send_stderr(buf);
            buf.len()
        })))
        .spawn(&linked, vec![]);

    let status_code = proc.proc.wait().await;
    status_code.check_success().context("Execution failed")?;
    Ok(())
}

pub async fn run_ls(cpp: bool, stdin: Rc<Pipe>, stdout: Rc<Pipe>, stderr: Rc<Pipe>) -> Result<()> {
    let std = match cpp {
        true => "-std=c++20",
        false => "-std=c17",
    };

    let mut fs = get_fs("cpp")
        .await
        .context("Failed to get C/C++ filesystem")?;
    let clangd = fs
        .get_file_with_path(b"bin/clangd")
        .context("Failed to get clangd executable")?;
    fs.add_file_with_path(
        b"compile_flags.txt",
        Rc::new(
            format!(
                r#"
-Wall
-O2
-I/include/c++/15.0.0/
-I/include/c++/15.0.0/wasm32-wasip1/
-resource-dir=/lib/clang/20
{std}
"#,
            )
            .into_bytes(),
        ),
    );
    let proc = ProcessHandle::builder()
        .fs(fs)
        .stdin(FdEntry::Pipe(stdin))
        .stdout(FdEntry::Pipe(stdout))
        .stderr(FdEntry::Pipe(stderr))
        .spawn(
            &clangd,
            vec![b"clangd".to_vec(), b"--pch-storage=memory".to_vec()],
        );

    crate::send_msg(common::WorkerMessage::LSReady);
    let status_code = proc.proc.wait().await;
    status_code.check_success().expect("LS process failed");
    Ok(())
}
