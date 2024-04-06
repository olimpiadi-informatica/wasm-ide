#![feature(stdarch_wasm_atomic_wait)]

use std::{
    collections::{HashMap, VecDeque},
    io::{BufRead, Read},
    sync::{
        atomic::{AtomicBool, Ordering},
        Condvar, Mutex, OnceLock,
    },
};

use anyhow::{Context, Result};
use async_channel::{unbounded, Receiver, Sender};

use common::{ClientMessage, Language, WorkerMessage};
use compiler::{LSInterface, RunnerInterface};
use log::{debug, info, warn};
use wasi::Fs;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

use crate::compiler::start_language_server;

mod compiler;
mod instrument;
mod thread;
mod wasi;

struct WorkerState {
    cache: Mutex<HashMap<Language, Fs>>,
    send_msg: Sender<WorkerMessage>,
    cancelled: AtomicBool,
    ls_cancelled: AtomicBool,
    ls_buffer: Mutex<VecDeque<u8>>,
    ls_notify: Condvar,
    ls_exited: Mutex<Option<Receiver<()>>>,
    ls_stderr_buffer: Mutex<VecDeque<u8>>,
    ls_stdout_buffer: Mutex<VecDeque<u8>>,
}

static WORKER_STATE: OnceLock<WorkerState> = OnceLock::new();

#[wasm_bindgen]
pub fn setup() {
    console_error_panic_hook::set_once();

    use tracing_subscriber::fmt::format::Pretty;
    use tracing_subscriber::prelude::*;
    use tracing_web::{performance_layer, MakeWebConsoleWriter};

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false) // Only partially supported across browsers
        .without_time() // std::time is not available in browsers, see note below
        .with_writer(MakeWebConsoleWriter::new()); // write events to the console
    let perf_layer = performance_layer().with_details_from_fields(Pretty::default());

    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(perf_layer)
        .init(); // Install these as subscribers to tracing events

    info!("worker started");

    let (s, r) = unbounded();

    // This message will only be sent once this function returns.
    s.try_send(WorkerMessage::Ready).unwrap();

    WORKER_STATE.get_or_init(|| WorkerState {
        cache: Mutex::new(HashMap::new()),
        send_msg: s,
        cancelled: AtomicBool::new(false),
        ls_cancelled: AtomicBool::new(false),
        ls_buffer: Mutex::new(VecDeque::new()),
        ls_notify: Condvar::new(),
        ls_exited: Mutex::new(None),
        ls_stderr_buffer: Mutex::new(VecDeque::new()),
        ls_stdout_buffer: Mutex::new(VecDeque::new()),
    });

    let worker = js_sys::global()
        .dyn_into::<DedicatedWorkerGlobalScope>()
        .expect("not a worker");

    worker.set_onmessage(Some(
        Closure::<dyn Fn(_)>::new(handle_message)
            .into_js_value()
            .unchecked_ref(),
    ));

    spawn_local(async move {
        let mut msg_num = 0;
        loop {
            let msg = r.recv().await.unwrap();
            if WORKER_STATE
                .get()
                .unwrap()
                .cancelled
                .load(Ordering::Relaxed)
            {
                if !matches!(msg, WorkerMessage::Done | WorkerMessage::Error(_)) {
                    // Skip queued output if we are quitting.
                    continue;
                }
            }
            msg_num += 1;
            const MSG_WAIT_COUNT: usize = 1000;
            if msg_num > MSG_WAIT_COUNT {
                // Wait 1ms every few messages to ensure that we have the opportunity to receive
                // the stop command.
                gloo_timers::future::TimeoutFuture::new(1).await;
                msg_num = 0;
            }
            let msg = serde_wasm_bindgen::to_value(&msg).expect("invalid message");
            worker.post_message(&msg).expect("main thread died");
        }
    });
}

fn send_msg(msg: WorkerMessage) {
    WORKER_STATE.get().unwrap().send_msg.try_send(msg).unwrap();
}

pub fn handle_message(msg: JsValue) {
    let msg = msg.dyn_into::<MessageEvent>().unwrap().data();
    let msg = serde_wasm_bindgen::from_value(msg).unwrap();
    let get_fs: fn(Language) -> _ = |l: Language| -> Result<Fs> {
        WORKER_STATE
            .get()
            .unwrap()
            .cache
            .lock()
            .unwrap()
            .get(&l)
            .cloned()
            .context("could not get fs for language")
    };
    let interface = RunnerInterface {
        should_stop: || {
            debug!("should stop?");
            let state = WORKER_STATE.get().unwrap();
            state.cancelled.load(Ordering::Relaxed)
        },
        send_stdout: |data: &[u8]| {
            debug!("send_stdout: {}", String::from_utf8_lossy(data));
            send_msg(WorkerMessage::StdoutChunk(data.to_owned()));
        },
        send_stderr: |data: &[u8]| {
            debug!("send_stderr: {}", String::from_utf8_lossy(data));
            send_msg(WorkerMessage::StderrChunk(data.to_owned()));
        },
        send_compiler_message: |data: &[u8]| {
            debug!("send_compiler_message: {}", String::from_utf8_lossy(data));
            send_msg(WorkerMessage::CompilationMessageChunk(data.to_owned()));
        },
        send_done: || {
            debug!("send_done");
            send_msg(WorkerMessage::Done);
        },
        send_compilation_done: || {
            debug!("send_compilation_done");
            send_msg(WorkerMessage::CompilationDone);
        },
        send_error: |s: String| {
            debug!("send_error: {s}");
            send_msg(WorkerMessage::Error(s));
        },
        get_fs,
    };
    let ls_interface = LSInterface {
        should_stop: || {
            debug!("ls should stop?");
            let state = WORKER_STATE.get().unwrap();
            state.ls_cancelled.load(Ordering::Relaxed)
        },
        recv_stdin: |buf: &mut [u8]| -> usize {
            debug!("ls recv stdin");
            let state = WORKER_STATE.get().unwrap();
            if state.ls_cancelled.load(Ordering::Relaxed) {
                return 0;
            }
            let mut dbuf = state.ls_buffer.lock().unwrap();
            let avail = dbuf.as_slices().0;
            let to_read = avail.len().min(buf.len());
            if to_read == 0 {
                drop(state.ls_notify.wait(dbuf).unwrap());
                return 0;
            }
            debug!("{}", String::from_utf8_lossy(avail));
            buf[..to_read].copy_from_slice(&avail[..to_read]);
            dbuf.drain(0..to_read);
            return to_read;
        },
        send_stdout: |data: &[u8]| {
            debug!("ls message: {:?}", &String::from_utf8_lossy(data));
            let state = WORKER_STATE.get().unwrap();
            let mut obuf = state.ls_stdout_buffer.lock().unwrap();
            obuf.extend(data);

            let mut backfill_buf = vec![];

            let mut content_length = 0;

            loop {
                let mut line = vec![];
                obuf.read_until(b'\n', &mut line).unwrap();
                backfill_buf.extend_from_slice(&line);

                if !line.ends_with(b"\n") {
                    break;
                }

                if line.starts_with(b"Content-Length: ") {
                    content_length = match String::from_utf8_lossy(&line[16..(line.len() - 2)])
                        .parse::<usize>()
                    {
                        Ok(len) => len,
                        Err(e) => {
                            warn!("Invalid ls content length: {e}");
                            continue;
                        }
                    };
                }

                if line == b"\r\n" {
                    if obuf.len() >= content_length {
                        let mut buf = vec![0; content_length];
                        obuf.read_exact(&mut buf).unwrap();
                        let message = String::from_utf8_lossy(&buf);
                        send_msg(WorkerMessage::LSMessage(message.to_string()));
                        content_length = 0;
                        backfill_buf.clear();
                    }
                }
            }

            for c in backfill_buf.iter().rev() {
                obuf.push_front(*c);
            }
        },
        send_stderr: |data: &[u8]| {
            let state = WORKER_STATE.get().unwrap();
            let mut ebuf = state.ls_stderr_buffer.lock().unwrap();
            let original_len = ebuf.len();
            ebuf.extend(data.iter());
            if let Some(pos) =
                data.iter()
                    .enumerate()
                    .rev()
                    .find_map(|(pos, x)| if *x == b'\n' { Some(pos) } else { None })
            {
                let pos = pos + original_len;
                ebuf.make_contiguous();
                let msg = &ebuf.as_slices().0[..pos];
                info!("ls stderr: {}", String::from_utf8_lossy(msg));
                ebuf.drain(0..pos + 1);
            }
        },
        get_fs,
        notify: || {
            let state = WORKER_STATE.get().unwrap();
            state.ls_notify.notify_all();
        },
    };
    match msg {
        ClientMessage::Compile {
            source,
            language,
            input,
            base_url,
        } => {
            spawn_local(async move {
                debug!("got msg: {base_url} {source} {language:?}");
                WORKER_STATE
                    .get()
                    .unwrap()
                    .cancelled
                    .store(false, Ordering::Relaxed);
                send_msg(WorkerMessage::Started);
                let cache = &WORKER_STATE.get().unwrap().cache;
                match compiler::prepare_cache(base_url, language, &mut cache.lock().unwrap()).await
                {
                    Ok(l) => l,
                    Err(e) => {
                        (interface.send_error)(format!("{:?}", e));
                        return;
                    }
                };
                send_msg(WorkerMessage::CompilerFetched);
                compiler::compile_one(source, language, input, interface).await;
            });
        }
        ClientMessage::Cancel => {
            WORKER_STATE
                .get()
                .unwrap()
                .cancelled
                .store(true, Ordering::Relaxed);
        }
        ClientMessage::StartLS(base_url, language) => {
            info!("start LS for {language:?}");
            spawn_local(async move {
                send_msg(WorkerMessage::LSStopping);
                let state = WORKER_STATE.get().unwrap();
                state.ls_cancelled.store(true, Ordering::Relaxed);
                state.ls_notify.notify_all();
                let maybe_receiver = state.ls_exited.lock().unwrap().clone();
                if let Some(receiver) = maybe_receiver {
                    let _ = receiver.recv().await;
                }
                state.ls_cancelled.store(false, Ordering::Relaxed);
                state.ls_buffer.lock().unwrap().clear();
                state.ls_stderr_buffer.lock().unwrap().clear();
                state.ls_stdout_buffer.lock().unwrap().clear();
                let cache = &WORKER_STATE.get().unwrap().cache;
                match compiler::prepare_cache(base_url, language, &mut cache.lock().unwrap()).await
                {
                    Ok(l) => l,
                    Err(e) => {
                        (interface.send_error)(format!("{:?}", e));
                        return;
                    }
                };
                send_msg(WorkerMessage::LSReady);
                let (sender, receiver) = unbounded();
                *state.ls_exited.lock().unwrap() = Some(receiver);
                let res = start_language_server(language, ls_interface).await;
                if let Err(e) = res {
                    warn!("Error running language server: {e}");
                    (interface.send_error)(format!("Error running language server: {e}"));
                }
                sender.send(()).await.unwrap();
                *state.ls_exited.lock().unwrap() = None;
            });
        }
        ClientMessage::LSMessage(message) => {
            debug!("{message}");
            let state = WORKER_STATE.get().unwrap();
            let mut dbuf = state.ls_buffer.lock().unwrap();
            let message = message.as_bytes();
            dbuf.extend(format!("Content-Length: {}\r\n\r\n", message.len()).into_bytes());
            dbuf.extend(message);
            state.ls_notify.notify_all();
        }
    }
}
