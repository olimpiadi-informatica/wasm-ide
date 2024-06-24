use std::sync::{Mutex, OnceLock};

use js_sys::Object;
use log::debug;
use send_wrapper::SendWrapper;
use wasm_bindgen::{prelude::*, JsValue};
use web_sys::{Worker, WorkerOptions, WorkerType};

static AVAILABLE_WORKERS: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();

struct WorkerData {
    worker: SendWrapper<Worker>,
    next_task: Option<Box<dyn FnOnce(JsValue) + Send + 'static>>,
}

static WORKER_DATA: OnceLock<Mutex<Vec<WorkerData>>> = OnceLock::new();

pub async fn spawn_simple<F>(closure: F)
where
    F: FnOnce() + Send + 'static,
{
    spawn(move |_| closure(), JsValue::null()).await;
}

pub async fn spawn<F>(closure: F, arg: JsValue)
where
    F: FnOnce(JsValue) + Send + 'static,
{
    debug!("spawning thread");

    let worker_id = {
        let available_worker = AVAILABLE_WORKERS
            .get_or_init(|| Mutex::new(vec![]))
            .lock()
            .unwrap()
            .pop();
        if let Some(w) = available_worker {
            w
        } else {
            let mut options = WorkerOptions::default();
            options.type_(WorkerType::Module);
            let worker = Worker::new_with_options("./start_worker_thread.js", &options)
                .expect("couldn't start thread");

            let mut msg = Object::new();
            js_sys::Reflect::set(&mut msg, &"module".into(), &wasm_bindgen::module())
                .expect("could not set module");
            js_sys::Reflect::set(&mut msg, &"memory".into(), &wasm_bindgen::memory())
                .expect("could not set memory");
            worker
                .post_message(&msg)
                .expect("failed sending init message to worker");
            let mut workers = WORKER_DATA
                .get_or_init(|| Mutex::new(vec![]))
                .lock()
                .unwrap();
            workers.push(WorkerData {
                worker: SendWrapper::new(worker),
                next_task: None,
            });
            workers.len() - 1
        }
    };

    let mut msg = Object::new();
    js_sys::Reflect::set(&mut msg, &"workerIndex".into(), &JsValue::from(worker_id))
        .expect("could not set worker_id");
    js_sys::Reflect::set(&mut msg, &"arg".into(), &arg).expect("could not set arg");

    let mut workers = WORKER_DATA
        .get_or_init(|| Mutex::new(vec![]))
        .lock()
        .unwrap();

    workers[worker_id].next_task = Some(Box::new(closure));
    workers[worker_id]
        .worker
        .post_message(&msg)
        .expect("failed sending task message to worker");
}

#[wasm_bindgen(js_name = "threadRunFn")]
pub fn thread_run_fun(worker_id: usize, arg: JsValue) {
    debug!("thread starting...");

    let closure = {
        let mut workers = WORKER_DATA
            .get_or_init(|| Mutex::new(vec![]))
            .lock()
            .unwrap();
        workers[worker_id]
            .next_task
            .take()
            .expect("sent message to worker without a closure to run")
    };

    closure(arg);

    AVAILABLE_WORKERS
        .get_or_init(|| Mutex::new(vec![]))
        .lock()
        .unwrap()
        .push(worker_id);
}
