use std::sync::{Arc, Mutex};

use anyhow::Result;
use common::{WorkerExecRequest, WorkerExecResponse, WorkerRequest};
use futures_channel::oneshot::{self, Sender};
use futures_util::{FutureExt, select};
use leptos::task::spawn_local;

use crate::backend::{Backend, Callback, Language};

pub struct RemoteBackend {
    address: String,
    languages: Vec<Language>,
    callback: Mutex<Option<Callback>>,
    stop: Mutex<Option<Sender<()>>>,
}

impl RemoteBackend {
    pub async fn new(address: String) -> Result<Arc<Self>> {
        let languages = api::languages(&address).await?;
        Ok(Arc::new(Self {
            address,
            languages,
            callback: Mutex::new(None),
            stop: Mutex::new(None),
        }))
    }
}

impl Backend for RemoteBackend {
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
                //unimplemented!("Language server is not supported in remote backend")
                return;
            }
        };

        match exec {
            WorkerExecRequest::CompileAndRun {
                files,
                primary_file,
                language,
                input,
                config,
            } => {
                let (sender, mut receiver) = oneshot::channel();
                self.stop.lock().unwrap().replace(sender);
                spawn_local(async move {
                    let future = api::evaluate(
                        &self.address,
                        None,
                        files
                            .into_iter()
                            .map(|f| (f.name, f.content.into_bytes()))
                            .collect(),
                        primary_file,
                        input.unwrap_or_default(),
                        config.time_limit,
                        config.mem_limit.map(|m| m as u64 / 16),
                        Some(language),
                    );

                    select! {
                        res = future.fuse() => {
                            let callback = self.callback.lock().unwrap();
                            let Some(callback) = callback.as_deref() else {
                                tracing::error!("No callback set for RemoteBackend");
                                return;
                            };
                            let res = match res {
                                Ok(res) => res,
                                Err(e) => {
                                    callback(WorkerExecResponse::Error(e.to_string()).into());
                                    return;
                                }
                            };

                            if let Some(compilation) = res.compilation {
                                callback(
                                    WorkerExecResponse::CompilationMessageChunk(compilation.stderr.into())
                                        .into(),
                                );
                                if compilation.status != "Success" {
                                    callback(
                                        WorkerExecResponse::Error(format!(
                                            "Compilation failed: {}",
                                            compilation.status
                                        ))
                                        .into(),
                                    );
                                    return;
                                }
                            }
                            if let Some(execution) = res.execution {
                                callback(WorkerExecResponse::StdoutChunk(execution.stdout.into()).into());
                                callback(WorkerExecResponse::StderrChunk(execution.stderr.into()).into());
                                if execution.status != "Success" {
                                    callback(
                                        WorkerExecResponse::Error(format!(
                                            "Execution failed: {}",
                                            execution.status
                                        ))
                                        .into(),
                                    );
                                    return;
                                }
                            }
                            callback(WorkerExecResponse::Success.into());
                        }
                        _ = receiver => {
                            let callback = self.callback.lock().unwrap();
                            let Some(callback) = callback.as_deref() else {
                                tracing::error!("No callback set for RemoteBackend");
                                return;
                            };
                            callback(WorkerExecResponse::Error("Execution cancelled by the user".to_string()).into());
                        }
                    }
                });
            }
            WorkerExecRequest::Cancel => {
                if let Some(stop) = self.stop.lock().unwrap().take() {
                    let _ = stop.send(());
                }
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

mod api {
    use anyhow::{Result, ensure};
    use gloo_net::http::Request;
    use serde::{Deserialize, Serialize};

    use super::Language;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(untagged)]
    pub enum Content {
        Text(String),
        Binary(Vec<u8>),
    }

    impl From<Vec<u8>> for Content {
        fn from(v: Vec<u8>) -> Self {
            match String::from_utf8(v) {
                Ok(s) => Content::Text(s),
                Err(e) => Content::Binary(e.into_bytes()),
            }
        }
    }

    impl From<Content> for Vec<u8> {
        fn from(c: Content) -> Self {
            match c {
                Content::Text(s) => s.into_bytes(),
                Content::Binary(b) => b,
            }
        }
    }

    #[derive(Debug, Deserialize)]
    #[allow(unused)]
    pub struct ExecutionResult {
        pub status: String,
        pub exit_code: u32,
        pub stdout: Content,
        pub stderr: Content,
        pub time: f64,
        pub memory: f64,
    }

    #[derive(Debug, Deserialize)]
    pub struct EvalRes {
        pub execution: Option<ExecutionResult>,
        pub compilation: Option<ExecutionResult>,
    }

    pub async fn languages(address: &str) -> Result<Vec<Language>> {
        let res = Request::get(&format!("{address}/languages")).send().await?;
        ensure!(res.ok(), "Failed to fetch languages: {}", res.status());
        Ok(res.json().await?)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn evaluate(
        address: &str,
        token: Option<String>,
        files: Vec<(String, Vec<u8>)>,
        main_filename: String,
        input: Vec<u8>,
        time_limit: Option<f64>,
        memory_limit: Option<u64>,
        language: Option<String>,
    ) -> Result<EvalRes> {
        #[derive(Debug, Serialize)]
        struct SourceFileContent {
            name: String,
            content: Content,
        }

        #[derive(Debug, Serialize)]
        struct EvalReq {
            files: Vec<SourceFileContent>,
            main_filename: String,
            input: Content,
            time_limit: Option<f64>,
            memory_limit: Option<u64>,
            language: Option<String>,
        }

        let req_body = EvalReq {
            files: files
                .into_iter()
                .map(|(name, content)| SourceFileContent {
                    name,
                    content: content.into(),
                })
                .collect(),
            main_filename,
            input: input.into(),
            time_limit,
            memory_limit,
            language,
        };

        let mut req = Request::post(&format!("{address}/evaluate"));
        if let Some(token) = token {
            req = req.header("Authorization", &format!("Bearer {token}"));
        }
        let req = req.json(&req_body)?;
        let res = req.send().await?;

        ensure!(res.ok(), "Failed to evaluate: {}", res.status());

        let res: EvalRes = res.json().await?;

        Ok(res)
    }
}
