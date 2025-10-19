use std::{cell::RefCell, rc::Rc};

use anyhow::{Context, Result};
use common::File;
use js_sys::WebAssembly::Module;

use crate::{
    os::{FdEntry, Fs, FsEntry, Pipe, ProcessHandle},
    util::*,
};

async fn compile(cpp: bool, llvm: Module, fs: Fs, code: Vec<u8>) -> Result<Vec<u8>> {
    let lang = match cpp {
        true => &b"c++"[..],
        false => &b"c"[..],
    };
    let std = match cpp {
        true => &b"-std=c++20"[..],
        false => &b"-std=c17"[..],
    };
    let compiled = Rc::new(RefCell::new(Vec::new()));
    let compiled2 = compiled.clone();
    let proc = ProcessHandle::builder()
        .fs(fs)
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
        .arg("clang++")
        .arg("-cc1")
        .arg("-isysroot")
        .arg("/")
        .arg("-I/include/c++/15.0.0/wasm32-wasip1/")
        .arg("-I/include/c++/15.0.0/")
        .arg("-stdlib=libstdc++")
        .arg("-internal-isystem")
        .arg("/lib/clang/20/include")
        .arg("-internal-isystem")
        .arg("/include/wasm32-wasip1-threads")
        .arg("-I/include/")
        .arg("-resource-dir")
        .arg("lib/clang/20")
        .arg("-target-feature")
        .arg("+atomics")
        .arg("-target-feature")
        .arg("+bulk-memory")
        .arg("-target-feature")
        .arg("+mutable-globals")
        .arg("-I.")
        .arg("-fcolor-diagnostics")
        .arg("-x")
        .arg(lang)
        .arg("-O2")
        .arg("-Wall")
        .arg(std)
        .arg("-emit-obj")
        .arg("-")
        .arg("-o")
        .arg("-")
        .spawn_with_module(llvm);

    let status_code = proc.proc.wait().await;
    status_code.check_success()?;
    let compiled = std::mem::take(&mut *compiled.borrow_mut());
    Ok(compiled)
}

async fn link(llvm: Module, mut fs: Fs, compiled: Vec<Vec<u8>>) -> Result<Vec<u8>> {
    let linked = Rc::new(RefCell::new(Vec::new()));
    let linked2 = linked.clone();
    let num_files = compiled.len();
    for (i, data) in compiled.into_iter().enumerate() {
        fs.add_file_with_path(format!("source{}.o", i).as_bytes(), Rc::new(data));
    }
    let proc = ProcessHandle::builder()
        .fs(fs)
        .stdout(FdEntry::WriteFn(Rc::new(move |buf: &[u8]| {
            linked2.borrow_mut().extend_from_slice(buf);
            buf.len()
        })))
        .stderr(FdEntry::WriteFn(Rc::new(move |buf: &[u8]| {
            send_compiler_message(buf);
            buf.len()
        })))
        .arg("wasm-ld")
        .arg("-L/lib/wasm32-wasip1-threads/")
        .arg("-lc")
        .arg("/lib/clang/20/lib/wasm32-unknown-wasip1-threads/libclang_rt.builtins.a")
        .arg("/lib/wasm32-wasip1-threads/crt1.o")
        .arg("-L/lib")
        .arg("-lstdc++")
        .arg("-lsupc++")
        .arg("-z")
        .arg("stack-size=16777216")
        .arg("--stack-first")
        .arg("--shared-memory")
        .arg("--import-memory")
        .arg("--export-memory")
        .arg("--max-memory=4294967296")
        .arg("-o")
        .arg("-")
        .args((0..num_files).map(|i| format!("source{}.o", i)))
        .spawn_with_module(llvm);

    let status_code = proc.proc.wait().await;
    status_code.check_success()?;
    let linked = std::mem::take(&mut *linked.borrow_mut());
    Ok(linked)
}

pub async fn run(cpp: bool, files: Vec<File>, stdin: Pipe, stdout: Pipe) -> Result<()> {
    send_fetching_compiler();
    let fs = get_fs("cpp")
        .await
        .context("Failed to get C/C++ filesystem")?;

    send_compiling();
    let llvm_exe = fs
        .get_file_with_path(b"/bin/llvm")
        .context("Failed to get clang executable")?;
    let uint8array = js_sys::Uint8Array::new_with_length(llvm_exe.len() as u32);
    uint8array.copy_from(&llvm_exe);
    let llvm_module = Module::new(&uint8array).expect("could not create module from wasm bytes");

    let mut compiled = Vec::new();
    for file in files {
        compiled.push(
            compile(
                cpp,
                llvm_module.clone(),
                fs.clone(),
                file.content.into_bytes(),
            )
            .await
            .context("Compilation failed")?,
        );
    }
    let linked = link(llvm_module, fs, compiled)
        .await
        .context("Linking failed")?;

    send_running();
    let mut fs = Fs::new();
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
        .spawn_with_code(&linked);

    let status_code = proc.proc.wait().await;
    status_code.check_success().context("Execution failed")?;
    Ok(())
}

pub async fn run_ls(cpp: bool, stdin: Pipe, stdout: Pipe, stderr: Pipe) -> Result<()> {
    let std = match cpp {
        true => "-std=c++20",
        false => "-std=c17",
    };

    crate::send_msg(common::WorkerLSResponse::FetchingCompiler);
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
        .arg("clangd")
        .arg("--pch-storage=memory")
        .spawn_with_code(&clangd);

    crate::send_msg(common::WorkerLSResponse::Started);
    let status_code = proc.proc.wait().await;
    status_code.check_success().context("clangd failed")?;
    Ok(())
}
