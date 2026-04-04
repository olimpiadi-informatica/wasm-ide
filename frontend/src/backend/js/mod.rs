use std::sync::{Arc, Mutex};

use common::{Language, WorkerExecRequest, WorkerExecResponse, WorkerRequest};
use gloo_timers::callback::Timeout;
use send_wrapper::SendWrapper;
use serde::Serialize;
use tracing::warn;
use wasm_bindgen::{JsCast, prelude::Closure};
use web_sys::{MessageEvent, Worker, WorkerOptions, WorkerType};

use crate::backend::{Backend, Callback};

struct ActiveExecution {
    worker: SendWrapper<Worker>,
    _timeout: Option<SendWrapper<Timeout>>,
}

pub struct JsBackend {
    languages: Vec<Language>,
    callback: Mutex<Option<Callback>>,
    execution: Mutex<Option<ActiveExecution>>,
}

impl JsBackend {
    pub async fn new() -> Arc<Self> {
        Arc::new(Self {
            languages: vec![Language {
                name: "JavaScript".to_string(),
                extensions: vec!["js".to_string(), "cjs".to_string(), "mjs".to_string()],
            }],
            callback: Mutex::new(None),
            execution: Mutex::new(None),
        })
    }

    fn finish_execution(&self) -> Option<Worker> {
        let execution = self.execution.lock().unwrap().take()?;
        Some(execution.worker.take())
    }

    fn callback(&self) -> Option<Callback> {
        self.callback.lock().unwrap().clone()
    }
}

impl Backend for JsBackend {
    fn languages(&self) -> &[Language] {
        &self.languages
    }

    fn set_callback(&self, callback: Callback) {
        self.callback.lock().unwrap().replace(callback);
    }

    fn send_message(self: Arc<Self>, msg: WorkerRequest) {
        let exec = match msg {
            WorkerRequest::Execution(exec) => exec,
            WorkerRequest::LS(_) => {
                // Language server is not supported in JS backend
                return;
            }
        };

        match exec {
            WorkerExecRequest::Run {
                files,
                primary_file,
                language: _,
                input,
                config,
            } => {
                let path = wasm_bindgen::link_to!(module = "/src/backend/js/worker.js");
                let options = WorkerOptions::default();
                options.set_type(WorkerType::Module);
                options.set_name("JS Worker");
                let worker =
                    Worker::new_with_options(&path, &options).expect("couldn't start thread");

                #[derive(Serialize)]
                struct Msg {
                    code: String,
                    input: Vec<u8>,
                }

                let code = files
                    .into_iter()
                    .find(|f| f.name == primary_file)
                    .expect("primary file not found")
                    .content;

                let msg = Msg {
                    code: String::from_utf8(code).expect("primary file is not valid UTF-8"),
                    input: input.expect("input is required for JS backend"),
                };

                worker.set_onmessage(Some(
                    Closure::<dyn Fn(_)>::new({
                        let this = self.clone();
                        move |msg: MessageEvent| {
                            let msg = serde_wasm_bindgen::from_value(msg.data());
                            let msg: WorkerExecResponse = match msg {
                                Ok(msg) => msg,
                                Err(e) => {
                                    warn!("invalid message from worker: {e}");
                                    return;
                                }
                            };

                            if matches!(
                                msg,
                                WorkerExecResponse::Success | WorkerExecResponse::Error(_)
                            ) {
                                if let Some(worker) = this.finish_execution() {
                                    worker.terminate();
                                } else {
                                    warn!("no worker to terminate");
                                    return;
                                }
                            }

                            let Some(callback) = this.callback() else {
                                warn!("no callback set for worker response");
                                return;
                            };
                            callback(msg.into());
                        }
                    })
                    .into_js_value()
                    .unchecked_ref(),
                ));

                let msg = serde_wasm_bindgen::to_value(&msg).expect("failed to serialize message");
                worker
                    .post_message(&msg)
                    .expect("failed to post message to worker");

                let timeout = config.time_limit.map(|time_limit| {
                    SendWrapper::new(Timeout::new((time_limit * 1000.) as _, {
                        let this = self.clone();
                        move || {
                            let Some(worker) = this.finish_execution() else {
                                return;
                            };
                            worker.terminate();
                            let Some(callback) = this.callback() else {
                                return;
                            };
                            callback(
                                WorkerExecResponse::Error("Execution timed out".to_string()).into(),
                            );
                        }
                    }))
                });

                self.execution.lock().unwrap().replace(ActiveExecution {
                    worker: SendWrapper::new(worker),
                    _timeout: timeout,
                });
            }
            WorkerExecRequest::Cancel => {
                let Some(worker) = self.finish_execution() else {
                    return;
                };
                worker.terminate();

                let Some(callback) = self.callback() else {
                    return;
                };
                callback(WorkerExecResponse::Error("Execution cancelled".to_string()).into());
            }
            WorkerExecRequest::StdinChunk(_) => {
                unimplemented!("Streaming stdin is not supported in remote backend")
            }
        }
    }

    fn has_dynamic_io(&self) -> bool {
        false
    }
}
