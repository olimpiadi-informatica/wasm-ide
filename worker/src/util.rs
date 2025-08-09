use std::{io::Read, rc::Rc};

use anyhow::{Context, Result};
use brotli_decompressor::BrotliDecompress;
use bytes::Bytes;
use tracing::debug;
use url::Url;
use wasm_bindgen::JsCast;
use web_sys::DedicatedWorkerGlobalScope;

use crate::{os::Fs, send_msg, WorkerMessage, WORKER_STATE};

async fn fetch_tarbr(name: &str) -> Result<Bytes> {
    let worker = js_sys::global()
        .dyn_into::<DedicatedWorkerGlobalScope>()
        .expect("not a worker");
    let base_url = worker.location().href();
    let url = Url::parse(&base_url)?.join(&format!("./compilers/{name}.tar.br"))?;
    let res = reqwest::get(url).await?;
    let res = res.error_for_status()?;
    let body = res.bytes().await?;
    Ok(body)
}

async fn get_fs_inner(name: &str) -> Result<Fs> {
    let body = fetch_tarbr(name)
        .await
        .with_context(|| format!("Failed to fetch compiler tarball for {name}"))?;
    let mut dec_body = vec![];
    BrotliDecompress(&mut &body[..], &mut dec_body).context("Error decompressing the tarball")?;
    let mut files = tar::Archive::new(&dec_body[..]);
    let mut fs = Fs::new();
    for x in files.entries()? {
        let mut x = x?;
        let path = x
            .path()?
            .to_string_lossy()
            .as_ref()
            .as_bytes()
            .strip_prefix(b".")
            .expect("invalid tarball")
            .to_vec();
        let mut contents = vec![];
        x.read_to_end(&mut contents)?;
        fs.add_file_with_path(&path, Rc::new(contents));
    }
    Ok(fs)
}

pub async fn get_fs(name: &str) -> Result<Fs> {
    let state = WORKER_STATE.get().expect("worker state not initialized");
    if let Some(fs) = state.fs_cache.borrow_mut().get(name).cloned() {
        return Ok(fs);
    }
    let fs = get_fs_inner(name).await?;
    state
        .fs_cache
        .borrow_mut()
        .insert(name.to_string(), fs.clone());
    Ok(fs)
}

pub fn send_fetching_compiler() {
    debug!("send_fetching_compiler");
    send_msg(WorkerMessage::Started);
}

pub fn send_compiling() {
    debug!("send_compiling");
    send_msg(WorkerMessage::CompilerFetched);
}

pub fn send_compiler_message(data: &[u8]) {
    debug!("send_compiler_message: {}", String::from_utf8_lossy(data));
    send_msg(WorkerMessage::CompilationMessageChunk(data.to_owned()));
}

pub fn send_running() {
    debug!("send_running");
    send_msg(WorkerMessage::CompilationDone);
}

pub fn send_stdout(data: &[u8]) {
    debug!("send_stdout: {}", String::from_utf8_lossy(data));
    send_msg(WorkerMessage::StdoutChunk(data.to_owned()));
}

pub fn send_stderr(data: &[u8]) {
    debug!("send_stderr: {}", String::from_utf8_lossy(data));
    send_msg(WorkerMessage::StderrChunk(data.to_owned()));
}
