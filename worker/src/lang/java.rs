use std::cell::RefCell;

use anyhow::{Context, Result, anyhow, bail};
use common::{ExecConfig, File};
use futures::channel::oneshot;
use futures::{FutureExt, select};
use gloo_timers::callback::Timeout;
use js_sys::{Array, Atomics, Int32Array, Object, Reflect, SharedArrayBuffer, Uint8Array};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::spawn_local;
use web_sys::{MessageEvent, Worker, WorkerOptions, WorkerType};

use crate::os::{Fs, Pipe, StatusCode};
use crate::util::*;

const STDIN_HEADER_BYTES: usize = 8;
const STDIN_CHUNK_BYTES: usize = 65536;

fn parse_package_name(source: &str) -> Option<String> {
    let mut in_block_comment = false;
    for raw_line in source.lines() {
        let mut line = raw_line.trim();
        loop {
            if in_block_comment {
                if let Some(idx) = line.find("*/") {
                    line = line[idx + 2..].trim();
                    in_block_comment = false;
                } else {
                    line = "";
                    break;
                }
            } else if let Some(rest) = line.strip_prefix("/*") {
                if let Some(idx) = rest.find("*/") {
                    line = rest[idx + 2..].trim();
                } else {
                    in_block_comment = true;
                    line = "";
                    break;
                }
            } else {
                break;
            }
        }
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("package")
            && rest.chars().next().is_some_and(char::is_whitespace)
            && let Some(pkg) = rest.split(';').next()
        {
            let pkg: String = pkg.chars().filter(|c| !c.is_whitespace()).collect();
            if !pkg.is_empty() {
                return Some(pkg);
            }
        }
        break;
    }
    None
}

fn expected_primary_class(primary_file: &str, primary_content: Option<&[u8]>) -> Option<String> {
    let stem = primary_file.strip_suffix(".java")?;
    let stem = stem.rsplit('/').next().unwrap_or(stem);
    let package = primary_content
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(parse_package_name);
    Some(match package {
        Some(pkg) => format!("{pkg}.{stem}"),
        None => stem.to_owned(),
    })
}

fn bytes_to_uint8_array(bytes: &[u8]) -> Uint8Array {
    let arr = Uint8Array::new_with_length(bytes.len() as u32);
    arr.copy_from(bytes);
    arr
}

struct WorkerGuard {
    worker: Worker,
    _onmessage: Option<Closure<dyn Fn(MessageEvent)>>,
    _onerror: Option<Closure<dyn Fn(wasm_bindgen::JsValue)>>,
}

impl WorkerGuard {
    fn new(worker: Worker) -> Self {
        Self {
            worker,
            _onmessage: None,
            _onerror: None,
        }
    }

    fn set_handlers(
        &mut self,
        onmessage: Closure<dyn Fn(MessageEvent)>,
        onerror: Closure<dyn Fn(wasm_bindgen::JsValue)>,
    ) {
        self.worker
            .set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        self.worker
            .set_onerror(Some(onerror.as_ref().unchecked_ref()));
        self._onmessage = Some(onmessage);
        self._onerror = Some(onerror);
    }
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.worker.set_onmessage(None);
        self.worker.set_onerror(None);
        self.worker.terminate();
    }
}

async fn compile_java(
    base_fs: &Fs,
    files: &[File],
    primary_class: Option<String>,
) -> Result<Vec<u8>> {
    let compiler_wasm = base_fs
        .get_file_with_path(b"/compiler.wasm")
        .context("Missing /compiler.wasm in java.tar")?;
    let runtime_js = base_fs
        .get_file_with_path(b"/compiler.wasm-runtime.js")
        .context("Missing /compiler.wasm-runtime.js in java.tar")?;
    let compile_sdk = base_fs
        .get_file_with_path(b"/compile-classlib-teavm.bin")
        .context("Missing /compile-classlib-teavm.bin in java.tar")?;
    let runtime_sdk = base_fs
        .get_file_with_path(b"/runtime-classlib-teavm.bin")
        .context("Missing /runtime-classlib-teavm.bin in java.tar")?;

    let path = wasm_bindgen::link_to!(module = "/src/lang/java_compiler.js");
    let options = WorkerOptions::default();
    options.set_type(WorkerType::Module);
    options.set_name("java-compiler");
    let worker = Worker::new_with_options(&path, &options)
        .map_err(|e| anyhow!("Failed to spawn Java compiler worker: {e:?}"))?;
    let mut guard = WorkerGuard::new(worker);

    let (tx, rx) = oneshot::channel::<Result<Vec<u8>>>();
    let tx = std::rc::Rc::new(RefCell::new(Some(tx)));
    let tx_msg = tx.clone();
    let tx_err = tx;

    let onmessage = Closure::<dyn Fn(MessageEvent)>::new(move |event: MessageEvent| {
        let data = event.data();
        let kind = Reflect::get(&data, &"kind".into())
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        match kind.as_str() {
            "diag" => {
                if let Some(text) = Reflect::get(&data, &"text".into())
                    .ok()
                    .and_then(|v| v.as_string())
                {
                    send_compiler_message(text.as_bytes());
                }
            }
            "ok" => {
                let res = match Reflect::get(&data, &"wasm".into())
                    .ok()
                    .and_then(|v| v.dyn_into::<Uint8Array>().ok())
                {
                    Some(arr) => Ok(arr.to_vec()),
                    None => Err(anyhow!("Compiler worker returned invalid Wasm payload")),
                };
                if let Some(tx) = tx_msg.borrow_mut().take() {
                    let _ = tx.send(res);
                }
            }
            "failed" | "error" => {
                let msg = Reflect::get(&data, &"message".into())
                    .ok()
                    .and_then(|v| v.as_string())
                    .unwrap_or_else(|| "Compilation failed".to_owned());
                if let Some(tx) = tx_msg.borrow_mut().take() {
                    let _ = tx.send(Err(anyhow!(msg)));
                }
            }
            _ => {}
        }
    });

    let onerror = Closure::<dyn Fn(wasm_bindgen::JsValue)>::new(move |err| {
        if let Some(tx) = tx_err.borrow_mut().take() {
            let _ = tx.send(Err(anyhow!("Java compiler worker error: {err:?}")));
        }
    });

    guard.set_handlers(onmessage, onerror);

    let js_files = Array::new();
    for f in files.iter().filter(|f| f.name.ends_with(".java")) {
        let content = std::str::from_utf8(&f.content)
            .with_context(|| format!("Java source file {} is not valid UTF-8", f.name))?;
        let obj = Object::new();
        Reflect::set(&obj, &"name".into(), &f.name.clone().into()).unwrap();
        Reflect::set(&obj, &"content".into(), &content.into()).unwrap();
        js_files.push(&obj);
    }

    let compiler_wasm_js = bytes_to_uint8_array(&compiler_wasm);
    let runtime_js_js = bytes_to_uint8_array(&runtime_js);
    let compile_sdk_js = bytes_to_uint8_array(&compile_sdk);
    let runtime_sdk_js = bytes_to_uint8_array(&runtime_sdk);

    let req = Object::new();
    Reflect::set(&req, &"compilerWasm".into(), &compiler_wasm_js).unwrap();
    Reflect::set(&req, &"runtimeJs".into(), &runtime_js_js).unwrap();
    Reflect::set(&req, &"compileSdk".into(), &compile_sdk_js).unwrap();
    Reflect::set(&req, &"runtimeSdk".into(), &runtime_sdk_js).unwrap();
    Reflect::set(&req, &"files".into(), &js_files).unwrap();
    if let Some(primary_class) = primary_class {
        Reflect::set(&req, &"primaryClass".into(), &primary_class.into()).unwrap();
    }

    let transfer = Array::new();
    transfer.push(&compiler_wasm_js.buffer());
    transfer.push(&runtime_js_js.buffer());
    transfer.push(&compile_sdk_js.buffer());
    transfer.push(&runtime_sdk_js.buffer());

    guard
        .worker
        .post_message_with_transfer(&req, &transfer)
        .map_err(|e| anyhow!("Failed to send compile request: {e:?}"))?;

    rx.await
        .map_err(|_| anyhow!("Java compiler worker terminated unexpectedly"))?
}

struct PipeCloseGuard(Pipe);

impl Drop for PipeCloseGuard {
    fn drop(&mut self) {
        self.0.close();
    }
}

async fn run_compiled_wasm(
    runtime_js: &[u8],
    app_wasm: &[u8],
    config: ExecConfig,
    stdin: Pipe,
    stdout: Pipe,
) -> Result<()> {
    let _stdout_guard = PipeCloseGuard(stdout);
    let path = wasm_bindgen::link_to!(module = "/src/lang/java_runner.js");
    let options = WorkerOptions::default();
    options.set_type(WorkerType::Module);
    options.set_name("java");
    let worker = Worker::new_with_options(&path, &options)
        .map_err(|e| anyhow!("Failed to spawn Java runner worker: {e:?}"))?;
    let mut guard = WorkerGuard::new(worker);

    let stdin_channel = SharedArrayBuffer::new((STDIN_HEADER_BYTES + STDIN_CHUNK_BYTES) as u32);
    let (tx, rx) = oneshot::channel::<StatusCode>();
    let tx = std::rc::Rc::new(RefCell::new(Some(tx)));
    let tx_msg = tx.clone();
    let tx_err = tx.clone();
    let tx_timeout = tx;

    let stdin_for_msg = stdin;
    let channel_for_msg = stdin_channel.clone();

    let onmessage = Closure::<dyn Fn(MessageEvent)>::new(move |event: MessageEvent| {
        let data = event.data();
        let kind = Reflect::get(&data, &"kind".into())
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        match kind.as_str() {
            "stdout" => {
                if let Ok(val) = Reflect::get(&data, &"data".into())
                    && let Ok(arr) = val.dyn_into::<Uint8Array>()
                {
                    let bytes = arr.to_vec();
                    send_stdout(&bytes);
                }
            }
            "stderr" => {
                if let Ok(val) = Reflect::get(&data, &"data".into())
                    && let Ok(arr) = val.dyn_into::<Uint8Array>()
                {
                    let bytes = arr.to_vec();
                    send_stderr(&bytes);
                }
            }
            "stdin_read" => {
                let stdin = stdin_for_msg.clone();
                let channel = channel_for_msg.clone();
                spawn_local(async move {
                    let mut buf = vec![0u8; STDIN_CHUNK_BYTES];
                    let n = stdin.read(&mut buf).await;
                    let ctrl = Int32Array::new_with_byte_offset_and_length(&channel, 0, 2);
                    if n > 0 {
                        let payload = Uint8Array::new_with_byte_offset_and_length(
                            &channel,
                            STDIN_HEADER_BYTES as u32,
                            n as u32,
                        );
                        payload.copy_from(&buf[..n]);
                        let _ = Atomics::store(&ctrl, 1, n as i32);
                    } else {
                        let _ = Atomics::store(&ctrl, 1, -1);
                    }
                    let _ = Atomics::store(&ctrl, 0, 1);
                    let _ = Atomics::notify(&ctrl, 0);
                });
            }
            "exit" => {
                let code = Reflect::get(&data, &"code".into())
                    .ok()
                    .and_then(|v| v.as_f64())
                    .map(|v| v as u32)
                    .unwrap_or(0);
                if let Some(tx) = tx_msg.borrow_mut().take() {
                    let _ = tx.send(StatusCode::Exited(code));
                }
            }
            _ => {}
        }
    });

    let onerror = Closure::<dyn Fn(wasm_bindgen::JsValue)>::new(move |err| {
        if let Some(tx) = tx_err.borrow_mut().take() {
            let _ = tx.send(StatusCode::RuntimeError(format!(
                "Java runner worker error: {err:?}"
            )));
        }
    });

    guard.set_handlers(onmessage, onerror);

    let _timeout = config.time_limit.map(|time_limit| {
        Timeout::new((time_limit * 1000.0) as u32, move || {
            if let Some(tx) = tx_timeout.borrow_mut().take() {
                let _ = tx.send(StatusCode::Signaled);
            }
        })
    });

    let init_msg = Object::new();
    let runtime_js_arr = bytes_to_uint8_array(runtime_js);
    let app_wasm_arr = bytes_to_uint8_array(app_wasm);
    Reflect::set(&init_msg, &"runtimeJs".into(), &runtime_js_arr).unwrap();
    Reflect::set(&init_msg, &"appWasm".into(), &app_wasm_arr).unwrap();
    Reflect::set(&init_msg, &"stdinChannel".into(), &stdin_channel).unwrap();

    let transfer = Array::new();
    transfer.push(&runtime_js_arr.buffer());
    transfer.push(&app_wasm_arr.buffer());

    guard
        .worker
        .post_message_with_transfer(&init_msg, &transfer)
        .map_err(|e| anyhow!("Failed to start Java runner worker: {e:?}"))?;

    let status = rx.await.unwrap_or(StatusCode::Signaled);
    drop(guard);
    drop(_stdout_guard);
    status.check_success().context("Execution failed")
}

pub async fn run(
    config: ExecConfig,
    files: Vec<File>,
    primary_file: String,
    stdin: Pipe,
    stdout: Pipe,
) -> Result<()> {
    send_fetching_compiler();
    let base_fs = get_fs("java")
        .await
        .context("Failed to get Java filesystem")?;

    send_compiling();
    if !files.iter().any(|f| f.name.ends_with(".java")) {
        bail!("No Java source files found");
    }

    let primary_content = files
        .iter()
        .find(|f| f.name == primary_file)
        .map(|f| f.content.as_slice());
    let primary_class = expected_primary_class(&primary_file, primary_content);

    let app_wasm = compile_java(&base_fs, &files, primary_class).await?;

    let runtime_js = base_fs
        .get_file_with_path(b"/compiler.wasm-runtime.js")
        .context("Missing /compiler.wasm-runtime.js in java.tar")?;

    send_running();
    run_compiled_wasm(&runtime_js, &app_wasm, config, stdin, stdout).await
}

pub async fn run_ls(files: Vec<File>, stdin: Pipe, stdout: Pipe, stderr: Pipe) -> Result<()> {
    let _stdout_guard = PipeCloseGuard(stdout.clone());
    let _stderr_guard = PipeCloseGuard(stderr);

    crate::send_msg(common::WorkerLSResponse::FetchingCompiler);
    let base_fs = get_fs("java")
        .await
        .context("Failed to get Java filesystem")?;

    let compiler_wasm = base_fs
        .get_file_with_path(b"/compiler.wasm")
        .context("Missing /compiler.wasm in java.tar")?;
    let runtime_js = base_fs
        .get_file_with_path(b"/compiler.wasm-runtime.js")
        .context("Missing /compiler.wasm-runtime.js in java.tar")?;
    let compile_sdk = base_fs
        .get_file_with_path(b"/compile-classlib-teavm.bin")
        .context("Missing /compile-classlib-teavm.bin in java.tar")?;

    let path = wasm_bindgen::link_to!(module = "/src/lang/java_ls.js");
    let options = WorkerOptions::default();
    options.set_type(WorkerType::Module);
    options.set_name("java-ls");
    let worker = Worker::new_with_options(&path, &options)
        .map_err(|e| anyhow!("Failed to spawn Java language server worker: {e:?}"))?;
    let mut guard = WorkerGuard::new(worker);

    let (ready_tx, ready_rx) = oneshot::channel::<Result<()>>();
    let (done_tx, done_rx) = oneshot::channel::<Result<()>>();
    let ready_tx = std::rc::Rc::new(RefCell::new(Some(ready_tx)));
    let done_tx = std::rc::Rc::new(RefCell::new(Some(done_tx)));
    let ready_tx_err = ready_tx.clone();
    let done_tx_err = done_tx.clone();
    let stdout_for_msg = stdout;

    let onmessage = Closure::<dyn Fn(MessageEvent)>::new(move |event: MessageEvent| {
        let data = event.data();
        let kind = Reflect::get(&data, &"kind".into())
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        match kind.as_str() {
            "ready" => {
                if let Some(tx) = ready_tx.borrow_mut().take() {
                    let _ = tx.send(Ok(()));
                }
            }
            "msg" => {
                if let Some(json) = Reflect::get(&data, &"data".into())
                    .ok()
                    .and_then(|v| v.as_string())
                {
                    let header = format!("Content-Length: {}\r\n\r\n", json.len());
                    stdout_for_msg.write(header.as_bytes());
                    stdout_for_msg.write(json.as_bytes());
                }
            }
            "exit" => {
                if let Some(tx) = done_tx.borrow_mut().take() {
                    let _ = tx.send(Ok(()));
                }
            }
            "error" => {
                let msg = Reflect::get(&data, &"message".into())
                    .ok()
                    .and_then(|v| v.as_string())
                    .unwrap_or_else(|| "Java language server error".to_owned());
                if let Some(tx) = ready_tx.borrow_mut().take() {
                    let _ = tx.send(Err(anyhow!(msg.clone())));
                } else if let Some(tx) = done_tx.borrow_mut().take() {
                    let _ = tx.send(Err(anyhow!(msg)));
                }
            }
            _ => {}
        }
    });

    let onerror = Closure::<dyn Fn(wasm_bindgen::JsValue)>::new(move |err| {
        let msg = format!("Java language server worker error: {err:?}");
        if let Some(tx) = ready_tx_err.borrow_mut().take() {
            let _ = tx.send(Err(anyhow!(msg)));
        } else if let Some(tx) = done_tx_err.borrow_mut().take() {
            let _ = tx.send(Err(anyhow!(msg)));
        }
    });

    guard.set_handlers(onmessage, onerror);

    let js_files = Array::new();
    for f in files.iter().filter(|f| f.name.ends_with(".java")) {
        if let Ok(content) = std::str::from_utf8(&f.content) {
            let obj = Object::new();
            Reflect::set(&obj, &"name".into(), &f.name.clone().into()).unwrap();
            Reflect::set(&obj, &"content".into(), &content.into()).unwrap();
            js_files.push(&obj);
        }
    }

    let compiler_wasm_js = bytes_to_uint8_array(&compiler_wasm);
    let runtime_js_js = bytes_to_uint8_array(&runtime_js);
    let compile_sdk_js = bytes_to_uint8_array(&compile_sdk);

    let init_msg = Object::new();
    Reflect::set(&init_msg, &"kind".into(), &"init".into()).unwrap();
    Reflect::set(&init_msg, &"compilerWasm".into(), &compiler_wasm_js).unwrap();
    Reflect::set(&init_msg, &"runtimeJs".into(), &runtime_js_js).unwrap();
    Reflect::set(&init_msg, &"compileSdk".into(), &compile_sdk_js).unwrap();
    Reflect::set(&init_msg, &"files".into(), &js_files).unwrap();

    let transfer = Array::new();
    transfer.push(&compiler_wasm_js.buffer());
    transfer.push(&runtime_js_js.buffer());
    transfer.push(&compile_sdk_js.buffer());

    guard
        .worker
        .post_message_with_transfer(&init_msg, &transfer)
        .map_err(|e| anyhow!("Failed to initialize Java language server worker: {e:?}"))?;

    ready_rx
        .await
        .map_err(|_| anyhow!("Java language server worker terminated during init"))??;

    crate::send_msg(common::WorkerLSResponse::Started);

    let mut done_rx = done_rx.fuse();
    let mut content_length = 0usize;
    let mut line = Vec::new();
    loop {
        select! {
            res = done_rx => {
                return res.unwrap_or(Ok(()));
            }
            _ = stdin.read_until(b'\n', &mut line).fuse() => {
                if line.is_empty() {
                    break;
                }
                if line.starts_with(b"Content-Length: ")
                    && let Some(len) = std::str::from_utf8(&line[16..])
                        .ok()
                        .and_then(|s| s.trim().parse::<usize>().ok())
                {
                    content_length = len;
                }
                if line == b"\r\n" {
                    let mut body = vec![0u8; content_length];
                    if stdin.read_exact(&mut body).await.is_err() {
                        break;
                    }
                    if let Ok(json) = String::from_utf8(body) {
                        let req = Object::new();
                        Reflect::set(&req, &"kind".into(), &"msg".into()).unwrap();
                        Reflect::set(&req, &"data".into(), &json.into()).unwrap();
                        guard
                            .worker
                            .post_message(&req)
                            .map_err(|e| anyhow!("Failed sending LSP message to Java LS worker: {e:?}"))?;
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::{expected_primary_class, parse_package_name};

    #[wasm_bindgen_test]
    fn test_parse_package_name() {
        assert_eq!(
            parse_package_name("// comment\n/* block */\npackage com.example.demo;\nclass A {}"),
            Some("com.example.demo".to_owned())
        );
        assert_eq!(parse_package_name("public class Main {}"), None);
    }

    #[wasm_bindgen_test]
    fn test_expected_primary_class() {
        assert_eq!(
            expected_primary_class("Main.java", Some(b"public class Main {}")),
            Some("Main".to_owned())
        );
        assert_eq!(
            expected_primary_class(
                "foo/bar/App.java",
                Some(b"package foo.bar;\npublic class App {}"),
            ),
            Some("foo.bar.App".to_owned())
        );
    }
}
