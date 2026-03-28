use std::collections::HashMap;
use std::sync::OnceLock;

use common::{WorkerRequest, WorkerResponse, init_logging};
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use futures::lock::Mutex;
use gloo_timers::future::TimeoutFuture;
use send_wrapper::SendWrapper;
use tracing::{info, warn};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::spawn_local;
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

use crate::exec::{WorkerStateExec, handle_exec_request};
use crate::ls::{WorkerStateLS, handle_ls_request};
use crate::os::Fs;

mod exec;
mod lang;
mod ls;
mod os;
mod util;

#[cfg(test)]
pub mod test;

struct WorkerState {
    send_msg: UnboundedSender<WorkerResponse>,
    fs_cache: Mutex<HashMap<String, Fs>>,
    exec: WorkerStateExec,
    ls: WorkerStateLS,
}

static WORKER_STATE: OnceLock<SendWrapper<WorkerState>> = OnceLock::new();

fn state() -> &'static WorkerState {
    WORKER_STATE.get().expect("worker state not initialized")
}

fn main() {
    init_logging();

    info!("Worker started");

    let (s, mut r) = unbounded();

    WORKER_STATE
        .set(SendWrapper::new(WorkerState {
            send_msg: s,
            fs_cache: Mutex::new(HashMap::new()),
            exec: WorkerStateExec::default(),
            ls: WorkerStateLS::default(),
        }))
        .ok()
        .expect("worker state already initialized");

    let worker = js_sys::global()
        .dyn_into::<DedicatedWorkerGlobalScope>()
        .expect("not a worker");

    worker.set_onmessage(Some(
        Closure::<dyn Fn(_)>::new(handle_message)
            .into_js_value()
            .unchecked_ref(),
    ));

    // This message will only be sent once this function returns.
    let msg = serde_wasm_bindgen::to_value(&lang::list()).expect("invalid message");
    worker.post_message(&msg).expect("main thread died");

    spawn_local(async move {
        let mut msg_num = 0;
        loop {
            let msg = r.next().await.expect("worker died?");
            msg_num += 1;
            const MSG_WAIT_COUNT: usize = 1000;
            if msg_num > MSG_WAIT_COUNT {
                // Wait 1ms every few messages to ensure that we have the opportunity to receive
                // the stop command.
                TimeoutFuture::new(1).await;
                msg_num = 0;
            }
            let msg = serde_wasm_bindgen::to_value(&msg).expect("invalid message");
            worker.post_message(&msg).expect("main thread died");
        }
    });
}

fn send_msg(msg: impl Into<WorkerResponse>) {
    let msg: WorkerResponse = msg.into();
    state()
        .send_msg
        .unbounded_send(msg)
        .expect("failed to send message");
}

fn handle_message(msg: JsValue) {
    let req = msg
        .dyn_into::<MessageEvent>()
        .expect("message event expected")
        .data();
    let req = match serde_wasm_bindgen::from_value::<WorkerRequest>(req) {
        Ok(req) => req,
        Err(e) => {
            warn!("Received invalid message: {e:?}");
            return;
        }
    };
    match req {
        WorkerRequest::Execution(req) => handle_exec_request(req),
        WorkerRequest::LS(req) => handle_ls_request(req),
    }
}
