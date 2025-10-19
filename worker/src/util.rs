use std::{collections::HashMap, io::Read, rc::Rc};

use anyhow::{anyhow, Context, Result};
use common::{WorkerExecResponse, WorkerExecStatus, WorkerResponse};
use futures::StreamExt;
use gloo_net::http::Request;
use tracing::{debug, info};
use wasm_bindgen_futures::stream::JsStream;

use crate::{os::Fs, send_msg, WORKER_STATE};

async fn manifest() -> Result<HashMap<String, u64>> {
    let res = Request::get("./compilers/manifest.json").send().await?;
    let manifest = res.json().await?;
    Ok(manifest)
}

async fn fetch_tar(name: &str) -> Result<Vec<u8>> {
    send_msg(WorkerResponse::FetchingCompiler(name.to_owned(), None));
    let url = format!("./compilers/{name}.tar");
    let res = Request::get(&url).send().await?;
    let reader = res.body().context("missing body")?;
    let mut js_stream = JsStream::from(reader.values());

    let manifest = manifest().await?;
    let size = *manifest.get(name).unwrap();

    let mut body = vec![];
    while let Some(chunk) = js_stream.next().await {
        let chunk = chunk.map_err(|_| anyhow!("stream error"))?;
        let chunk = js_sys::Uint8Array::new(&chunk).to_vec();
        body.extend_from_slice(&chunk);
        send_msg(WorkerResponse::FetchingCompiler(
            name.to_owned(),
            Some((body.len() as u64, size)),
        ));
    }

    send_msg(WorkerResponse::CompilerFetchDone(name.to_owned()));
    Ok(body)
}

pub fn fs_from_tar(tar: &[u8]) -> Result<Fs> {
    let mut files = tar::Archive::new(tar);
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

async fn get_fs_inner(name: &str) -> Result<Fs> {
    info!("Fetching {name}.tar");
    let body = fetch_tar(name)
        .await
        .with_context(|| format!("Failed to fetch compiler tarball for {name}"))?;
    let fs =
        fs_from_tar(&body).with_context(|| format!("Failed to deserialize tarball for {name}"))?;
    Ok(fs)
}

pub async fn get_fs(name: &str) -> Result<Fs> {
    let state = WORKER_STATE.get().expect("worker state not initialized");
    let mut fs_cache = state.fs_cache.lock().await;
    if let Some(fs) = fs_cache.get(name).cloned() {
        return Ok(fs);
    }
    let fs = get_fs_inner(name).await?;
    fs_cache.insert(name.to_string(), fs.clone());
    Ok(fs)
}

pub fn send_fetching_compiler() {
    debug!("send_fetching_compiler");
    send_msg(WorkerExecResponse::Status(
        WorkerExecStatus::FetchingCompiler,
    ));
}

pub fn send_compiling() {
    debug!("send_compiling");
    send_msg(WorkerExecResponse::Status(WorkerExecStatus::Compiling));
}

pub fn send_compiler_message(data: &[u8]) {
    debug!("send_compiler_message: {:?}", String::from_utf8_lossy(data));
    send_msg(WorkerExecResponse::CompilationMessageChunk(data.to_owned()));
}

pub fn send_running() {
    debug!("send_running");
    send_msg(WorkerExecResponse::Status(WorkerExecStatus::Running));
}

pub fn send_stdout(data: &[u8]) {
    debug!("send_stdout: {:?}", String::from_utf8_lossy(data));
    send_msg(WorkerExecResponse::StdoutChunk(data.to_owned()));
}

pub fn send_stderr(data: &[u8]) {
    debug!("send_stderr: {:?}", String::from_utf8_lossy(data));
    send_msg(WorkerExecResponse::StderrChunk(data.to_owned()));
}
