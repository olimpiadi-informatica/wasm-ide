use js_sys::Object;
use log::{debug, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use web_sys::{DedicatedWorkerGlobalScope, Worker, WorkerOptions, WorkerType};

pub async fn spawn_simple<F>(closure: F)
where
    F: FnOnce() + Send + 'static,
{
    spawn(move |_| closure(), JsValue::null()).await;
}

pub async fn spawn<F>(closure: F, arg: JsValue) -> Worker
where
    F: FnOnce(JsValue) + Send + 'static,
{
    debug!("spawning thread");
    let fun: Box<dyn FnOnce(JsValue) + Send + 'static> = Box::new(closure);

    let mut options = WorkerOptions::default();
    options.type_(WorkerType::Module);
    let worker = Worker::new_with_options("./start_worker_thread.js", &options)
        .expect("couldn't start thread");

    let mut msg = Object::new();
    js_sys::Reflect::set(&mut msg, &"module".into(), &wasm_bindgen::module())
        .expect("could not set module");
    js_sys::Reflect::set(&mut msg, &"memory".into(), &wasm_bindgen::memory())
        .expect("could not set memory");
    // This variable must stay alive until we receive a message from the worker.
    let fun: *mut dyn FnOnce(JsValue) = Box::into_raw(fun);
    js_sys::Reflect::set(
        &mut msg,
        &"closure".into(),
        &JsValue::from(&fun as *const _ as usize),
    )
    .expect("could not set closure");
    js_sys::Reflect::set(&mut msg, &"arg".into(), &arg).expect("could not set arg");
    let (send, recv) = async_channel::bounded(1);
    worker.set_onmessage(Some(
        Closure::<dyn Fn(_)>::new(move |_: JsValue| {
            debug!("thread started");
            if let Err(_) = send.try_send(()) {
                warn!("got multiple messages from spawned thread");
            }
        })
        .into_js_value()
        .unchecked_ref(),
    ));
    worker
        .post_message(&msg)
        .expect("could not send message to worker");
    recv.recv().await.expect("thread failed to start");
    worker
}

// # Safety
// Must be called in a worker spawned by `spawn`.
// `closure` must be the address of a pointer to a &mut dyn Fn(JsValue) + Send + 'static,
// which must stay alive until a message is sent back from the worker.
// The &mut dyn Fn(JsValue) + Send + 'static must be obtained via Box::leak.
#[wasm_bindgen(js_name = "threadRunFn")]
pub unsafe fn thread_run_fun(closure: usize, arg: JsValue) {
    debug!("thread starting...");
    let closure = (closure as *const &mut dyn FnOnce(JsValue)).read();
    let closure = Box::from_raw(closure);

    js_sys::global()
        .dyn_into::<DedicatedWorkerGlobalScope>()
        .expect("not a worker")
        .post_message(&JsValue::null())
        .expect("could not signal readiness");

    closure(arg)
}
