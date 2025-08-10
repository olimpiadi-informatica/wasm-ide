use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::OnceLock};

use common::{init_logging, ClientMessage, WorkerMessage};
use futures::{
    channel::{
        mpsc::{unbounded, UnboundedSender},
        oneshot::{channel, Sender},
    },
    select, FutureExt, StreamExt,
};
use os::{FdEntry, Fs, Pipe};
use send_wrapper::SendWrapper;
use tracing::{debug, error, info, warn};
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use wasm_bindgen_futures::spawn_local;
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

mod lang;
mod os;
mod util;

struct WorkerState {
    send_msg: UnboundedSender<WorkerMessage>,
    fs_cache: RefCell<HashMap<String, Fs>>,

    stdin: RefCell<Option<Rc<Pipe>>>,
    stop: RefCell<Option<Sender<()>>>,

    ls_stdin: RefCell<Option<Rc<Pipe>>>,
    ls_stop: RefCell<Option<Sender<()>>>,
}

static WORKER_STATE: OnceLock<SendWrapper<WorkerState>> = OnceLock::new();

fn worker_state() -> &'static WorkerState {
    WORKER_STATE.get().expect("worker state not initialized")
}

fn main() {
    init_logging();

    info!("Worker started");

    let (s, mut r) = unbounded();

    WORKER_STATE.get_or_init(|| {
        SendWrapper::new(WorkerState {
            send_msg: s,
            stdin: RefCell::new(None),
            stop: RefCell::new(None),
            ls_stdin: RefCell::new(None),
            ls_stop: RefCell::new(None),
            fs_cache: RefCell::new(HashMap::new()),
        })
    });

    // This message will only be sent once this function returns.
    send_msg(WorkerMessage::Ready);

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
            let msg = r.next().await.expect("worker died?");
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
    worker_state()
        .send_msg
        .unbounded_send(msg)
        .expect("failed to send message");
}

fn handle_message(msg: JsValue) {
    let msg = msg
        .dyn_into::<MessageEvent>()
        .expect("message event expected")
        .data();
    let msg = match serde_wasm_bindgen::from_value::<ClientMessage>(msg) {
        Ok(msg) => msg,
        Err(e) => {
            warn!("Received invalid message: {e:?}");
            return;
        }
    };

    match msg {
        ClientMessage::CompileAndRun {
            source,
            language,
            input,
        } => {
            info!("Starting execution of {:?} code", language);

            let stdin = Rc::new(Pipe::new());
            if let Some(input) = input {
                stdin.write(&input);
            }
            worker_state().stdin.borrow_mut().replace(stdin.clone());
            let (sender, mut receiver) = channel();
            worker_state().stop.borrow_mut().replace(sender);
            spawn_local(async move {
                let running = lang::run(language, source.into_bytes(), FdEntry::Pipe(stdin));
                select! {
                    _ = receiver => {
                        info!("Received stop command, cancelling execution");
                        send_msg(WorkerMessage::Error("Execution cancelled by user".to_string()));
                    }
                    res = running.fuse() => {
                        info!("Execution finished");
                        match res {
                            Ok(()) => send_msg(WorkerMessage::Done),
                            Err(e) => send_msg(WorkerMessage::Error(format!("{e:?}"))),
                        }
                    }
                };
            });
        }

        ClientMessage::StdinChunk(chunk) => {
            if let Some(stdin) = &*worker_state().stdin.borrow_mut() {
                stdin.write(&chunk);
            } else {
                warn!("Received stdin chunk but no pipe is set");
            }
        }

        ClientMessage::Cancel => {
            if let Some(s) = worker_state().stop.borrow_mut().take() {
                let _ = s.send(());
            } else {
                warn!("Received cancel message but no execution is running");
            }
            if worker_state().stdin.borrow_mut().take().is_some() {
                // TODO(virv): send EOF to the pipe
            }
        }

        ClientMessage::StartLS(lang) => {
            send_msg(WorkerMessage::LSStopping);
            if let Some(s) = worker_state().ls_stop.borrow_mut().take() {
                let _ = s.send(());
            }

            info!("Starting LS for {:?}", lang);

            let stdin = Rc::new(Pipe::new());
            worker_state().ls_stdin.borrow_mut().replace(stdin.clone());
            let stdout = Rc::new(Pipe::new());
            let stderr = Rc::new(Pipe::new());
            let (sender, mut receiver) = channel();
            worker_state().ls_stop.borrow_mut().replace(sender);

            spawn_local({
                let stdout = stdout.clone();
                let stderr = stderr.clone();
                async move {
                    let running = lang::run_ls(lang, stdin, stdout, stderr);
                    select! {
                        _ = receiver => {
                            info!("Received stop command, stopping LS");
                        }
                        res = running.fuse() => {
                            info!("LS finished");
                            match res {
                                Ok(()) => {}
                                Err(e) => error!("LS error: {e:?}"),
                            }
                        }
                    }
                }
            });

            spawn_local(async move {
                let mut content_length = 0usize;
                let mut line = Vec::new();
                loop {
                    stdout.read_until(b'\n', &mut line).await;
                    if line.is_empty() {
                        break;
                    }
                    if line.last() != Some(&b'\n') {
                        warn!("Partial message from LS");
                        continue;
                    }
                    if line.starts_with(b"Content-Length: ") {
                        content_length = std::str::from_utf8(&line[16..line.len() - 2])
                            .ok()
                            .and_then(|s| s.parse::<usize>().ok())
                            .expect("Invalid Content-Length");
                    }
                    if line == b"\r\n" {
                        line.resize(content_length, 0);
                        if stdout.read_exact(&mut line).await.is_err() {
                            warn!("Partial message from LS");
                            break;
                        }
                        let msg = String::from_utf8(line.clone()).unwrap();
                        send_msg(WorkerMessage::LSMessage(msg));
                    }
                }
            });

            spawn_local(async move {
                let mut line = Vec::new();
                loop {
                    stderr.read_until(b'\n', &mut line).await;
                    if line.is_empty() {
                        break;
                    }
                    if line.last() != Some(&b'\n') {
                        warn!("Partial line from LS stderr");
                        continue;
                    }
                    let msg = String::from_utf8_lossy(&line[..line.len() - 1]);
                    debug!("LS stderr: {}", msg);
                }
            });
        }

        ClientMessage::LSMessage(msg) => {
            if let Some(stdin) = &*worker_state().ls_stdin.borrow_mut() {
                debug!("Sending LS message: {}", msg);
                stdin.write(format!("Content-Length: {}\r\n\r\n", msg.len()).as_bytes());
                stdin.write(msg.as_bytes());
            } else {
                warn!("Received LS message but no pipe is set");
            }
        }
    }
}
