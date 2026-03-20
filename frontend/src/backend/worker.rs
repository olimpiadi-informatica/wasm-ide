use std::{
    cell::Cell,
    sync::{Arc, Mutex},
};

use common::{Language, WorkerRequest, WorkerResponse};
use send_wrapper::SendWrapper;
use tracing::warn;
use wasm_bindgen::{JsCast, JsValue, prelude::Closure};
use web_sys::{MessageEvent, Worker, WorkerOptions, WorkerType};

use crate::backend::{Backend, Callback};

pub struct WorkerBackend {
    languages: Vec<Language>,
    worker: SendWrapper<Worker>,
    callback: Mutex<Option<Callback>>,
}

impl WorkerBackend {
    pub async fn new() -> Arc<Self> {
        let options = WorkerOptions::default();
        options.set_type(WorkerType::Module);
        let worker = Worker::new_with_options("./worker_loader.js", &options)
            .expect("could not start worker");

        let (send, recv) = futures_channel::oneshot::channel();
        let send = Cell::new(Some(send));

        worker.set_onmessage(Some(
            Closure::<dyn Fn(_)>::new(move |msg: JsValue| {
                let msg = msg.dyn_into::<MessageEvent>().unwrap().data();
                let msg = match serde_wasm_bindgen::from_value::<Vec<Language>>(msg) {
                    Ok(msg) => msg,
                    Err(e) => {
                        warn!("invalid message from worker: {e}");
                        return;
                    }
                };
                let Some(send) = send.take() else {
                    warn!("unexpected message from worker: {msg:?}");
                    return;
                };
                send.send(msg).unwrap();
            })
            .into_js_value()
            .unchecked_ref(),
        ));

        let languages = recv.await.expect("worker failed to start");

        let this = Arc::new(Self {
            languages,
            worker: SendWrapper::new(worker.clone()),
            callback: Mutex::new(None),
        });

        worker.set_onmessage(Some(
            Closure::<dyn Fn(_)>::new({
                let this = this.clone();
                move |msg: JsValue| {
                    let msg = msg.dyn_into::<MessageEvent>().unwrap().data();
                    let msg = match serde_wasm_bindgen::from_value::<WorkerResponse>(msg) {
                        Ok(msg) => msg,
                        Err(e) => {
                            warn!("invalid message from worker: {e}");
                            return;
                        }
                    };

                    let callback = this.callback.lock().unwrap();
                    let Some(callback) = callback.as_deref() else {
                        return;
                    };

                    callback(msg);
                }
            })
            .into_js_value()
            .unchecked_ref(),
        ));

        this
    }
}

impl Backend for WorkerBackend {
    fn languages(&self) -> &[Language] {
        &self.languages
    }

    fn set_callback(&self, callback: Callback) {
        self.callback.lock().unwrap().replace(callback);
    }

    fn send_message(self: Arc<Self>, msg: WorkerRequest) {
        let js_msg = serde_wasm_bindgen::to_value(&msg).expect("invalid message to worker");
        self.worker.post_message(&js_msg).expect("worker died");
    }

    fn has_dynamic_io(&self, _lang: &str) -> bool {
        true
    }
}
