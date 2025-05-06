use std::{cell::RefCell, rc::Rc};

use anyhow::{Context, Result};

use crate::{
    os::{FileEntry, Fs, ProcessHandle},
    util::*,
};

async fn compile(cpp: bool, fs: Fs, code: Vec<u8>) -> Option<Vec<u8>> {
    let lang = match cpp {
        true => &b"c++"[..],
        false => &b"c"[..],
    };
    let std = match cpp {
        true => &b"-std=c++20"[..],
        false => &b"-std=c17"[..],
    };
    let exe = fs
        .get_file(fs.get(fs.root(), b"/bin/clang++").unwrap())
        .unwrap();
    let compiled = Rc::new(RefCell::new(Vec::new()));
    let compiled2 = compiled.clone();
    let proc = ProcessHandle::builder()
        .fs(fs.clone())
        .stdin(FileEntry::Data {
            data: code,
            offset: 0,
        })
        .stdout(FileEntry::WriteFn(Rc::new(move |buf: &[u8]| {
            compiled2.borrow_mut().extend_from_slice(buf);
            buf.len()
        })))
        .stderr(FileEntry::WriteFn(Rc::new(move |buf: &[u8]| {
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
                b"-I/include/c++/15.0.0/wasm32-wasi/".to_vec(),
                b"-I/include/c++/15.0.0/".to_vec(),
                b"-stdlib=libstdc++".to_vec(),
                b"-internal-isystem".to_vec(),
                b"/lib/clang/19/include".to_vec(),
                b"-internal-isystem".to_vec(),
                b"/include/wasm32-wasi-threads".to_vec(),
                b"-I/include/".to_vec(),
                b"-resource-dir".to_vec(),
                b"lib/clang/19".to_vec(),
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

    proc.proc.wait().await;
    if proc.proc.inner.borrow().status_code == Some(0) {
        let compiled = std::mem::take(&mut *compiled.borrow_mut());
        Some(compiled)
    } else {
        None
    }
}

async fn link(mut fs: Fs, compiled: Vec<u8>) -> Option<Vec<u8>> {
    let exe = fs
        .get_file(fs.get(fs.root(), b"/bin/wasm-ld").unwrap())
        .unwrap();
    let linked = Rc::new(RefCell::new(Vec::new()));
    let linked2 = linked.clone();
    fs.add_file_with_path(b"source.o", Rc::new(compiled));
    let proc = ProcessHandle::builder()
        .fs(fs.clone())
        .stdout(FileEntry::WriteFn(Rc::new(move |buf: &[u8]| {
            linked2.borrow_mut().extend_from_slice(buf);
            buf.len()
        })))
        .stderr(FileEntry::WriteFn(Rc::new(move |buf: &[u8]| {
            send_compiler_message(buf);
            buf.len()
        })))
        .spawn(
            &exe,
            vec![
                b"wasm-ld".to_vec(),
                b"-L/lib/wasm32-wasi-threads/".to_vec(),
                b"-lc".to_vec(),
                b"/lib/clang/19/lib/wasi/libclang_rt.builtins-wasm32.a".to_vec(),
                b"/lib/wasm32-wasi-threads/crt1.o".to_vec(),
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

    proc.proc.wait().await;
    if proc.proc.inner.borrow().status_code == Some(0) {
        let linked = std::mem::take(&mut *linked.borrow_mut());
        Some(linked)
    } else {
        None
    }
}

pub async fn run(cpp: bool, code: Vec<u8>, input: FileEntry) -> Result<()> {
    send_fetching_compiler();
    let fs = get_fs("cpp")
        .await
        .context("failed to get C/C++ filesystem")?;

    send_compiling();
    let compiled = compile(cpp, fs.clone(), code)
        .await
        .context("failed to compile code")?;
    let linked = link(fs, compiled)
        .await
        .context("failed to link compiled code")?;

    send_running();
    let proc = ProcessHandle::builder()
        .stdin(input)
        .stdout(FileEntry::WriteFn(Rc::new(move |buf: &[u8]| {
            send_stdout(buf);
            buf.len()
        })))
        .stderr(FileEntry::WriteFn(Rc::new(move |buf: &[u8]| {
            send_stderr(buf);
            buf.len()
        })))
        .spawn(&linked, vec![]);

    proc.proc.wait().await;
    Ok(())
}
