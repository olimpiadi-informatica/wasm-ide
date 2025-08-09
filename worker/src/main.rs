#![feature(stdarch_wasm_atomic_wait)]

use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::OnceLock};

use common::{ClientMessage, WorkerMessage};
use futures::{
    channel::{
        mpsc::{unbounded, UnboundedSender},
        oneshot::{channel, Sender},
    },
    select, FutureExt, StreamExt,
};
use os::{FileEntry, Fs, Pipe};
use send_wrapper::SendWrapper;
use tracing::{info, warn};
use tracing_subscriber::fmt::format::Pretty;
use tracing_subscriber::prelude::*;
use tracing_web::{performance_layer, MakeWebConsoleWriter};
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use wasm_bindgen_futures::spawn_local;
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

mod lang;
mod os;
mod util;

struct WorkerState {
    send_msg: UnboundedSender<WorkerMessage>,
    pipe: RefCell<Option<Rc<Pipe>>>,
    stop: RefCell<Option<Sender<()>>>,
    fs_cache: RefCell<HashMap<String, Fs>>,
}

static WORKER_STATE: OnceLock<SendWrapper<WorkerState>> = OnceLock::new();

fn main() {
    console_error_panic_hook::set_once();

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

    let (s, mut r) = unbounded();

    WORKER_STATE.get_or_init(|| {
        SendWrapper::new(WorkerState {
            send_msg: s,
            pipe: RefCell::new(None),
            stop: RefCell::new(None),
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
            let msg = r.next().await.unwrap();
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
    WORKER_STATE
        .get()
        .unwrap()
        .send_msg
        .unbounded_send(msg)
        .expect("failed to send message");
}

fn handle_message(msg: JsValue) {
    let msg = msg.dyn_into::<MessageEvent>().unwrap().data();
    let msg = serde_wasm_bindgen::from_value(msg).unwrap();

    match msg {
        ClientMessage::CompileAndRun {
            source,
            language,
            input,
        } => {
            let state = WORKER_STATE.get().unwrap();
            let pipe = Rc::new(Pipe::new());
            if let Some(input) = input {
                pipe.write(&input);
            }
            state.pipe.borrow_mut().replace(pipe.clone());
            let (sender, mut receiver) = channel();
            state.stop.borrow_mut().replace(sender);
            spawn_local(async move {
                let running = lang::run(language, source.into_bytes(), FileEntry::Pipe(pipe));
                select! {
                    _ = receiver => {
                        info!("Received stop command, cancelling execution");
                        send_msg(WorkerMessage::Error("Execution cancelled".to_string()));
                    }
                    res = running.fuse() => {
                        info!("Execution finished");
                        match res {
                            Ok(()) => send_msg(WorkerMessage::Done),
                            Err(e) => send_msg(WorkerMessage::Error(e.to_string())),
                        }
                    }
                };
            });
        }

        ClientMessage::StdinChunk(chunk) => {
            if let Some(pipe) = &*WORKER_STATE.get().unwrap().pipe.borrow_mut() {
                pipe.write(&chunk);
            } else {
                warn!("Received stdin chunk but no pipe is set");
            }
        }

        ClientMessage::Cancel => {
            let state = WORKER_STATE.get().unwrap();
            if let Some(s) = state.stop.borrow_mut().take() {
                let _ = s.send(());
            }
            state.pipe.borrow_mut().take();
        }

        ClientMessage::StartLS(_) => {}

        ClientMessage::LSMessage(_) => {}
    }
}
