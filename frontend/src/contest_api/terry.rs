use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use common::{
    ExecConfig, File, WorkerExecRequest, WorkerExecResponse, WorkerResponse, config::Workspace,
};
use futures_util::StreamExt;
use leptos::prelude::*;
use send_wrapper::SendWrapper;

use crate::{
    RunState, StateSubmit, backend,
    contest_api::{ContestAPI, SubmitStatus, Task},
};

pub struct Terry {
    path: String,
}

impl Terry {
    pub fn new(path: String) -> Self {
        Self { path }
    }
}

#[async_trait]
impl ContestAPI for Terry {
    async fn list_tasks(&self) -> Result<Vec<Task>> {
        Ok(SendWrapper::new(api::status(&self.path)).await.map(|res| {
            res.contest
                .tasks
                .unwrap_or_default()
                .into_iter()
                .map(|task| Task {
                    id: task.name.clone(),
                    name: task.title,
                })
                .collect()
        })?)
    }

    async fn init_workspace(&self, task: &str, lang: &str) -> Result<Workspace> {
        let lang_ext = backend::languages()
            .iter()
            .find(|l| l.name == lang)
            .map(|l| l.extensions[0].clone())
            .unwrap();

        let filename = format!("{task}.{lang_ext}");
        let source = SendWrapper::new(api::get_statement_file(&self.path, task, &filename)).await?;

        let input_filename = format!("{task}.input.txt");
        let input =
            SendWrapper::new(api::get_statement_file(&self.path, task, &input_filename)).await?;

        let code = [(filename, source.into())].into_iter().collect();
        let stdin = [(input_filename, input.into())].into_iter().collect();
        Ok(Workspace { code, stdin })
    }

    async fn task_score(&self, task: &str) -> Result<(f64, f64)> {
        let status = SendWrapper::new(api::status(&self.path)).await?;
        let max_score = status
            .contest
            .tasks
            .unwrap_or_default()
            .into_iter()
            .find(|contest_task| contest_task.name == task)
            .map(|contest_task| contest_task.max_score)
            .unwrap_or(0.0);
        let score = status
            .user
            .and_then(|user| user.tasks.get(task).map(|task| task.score))
            .unwrap_or(0.0);
        Ok((score, max_score))
    }

    async fn submit(
        &self,
        task: &str,
        language: &str,
        primary_file: &str,
        files: Vec<(String, Vec<u8>)>,
    ) -> Result<SubmitStatus> {
        let input = if let Some(input) = SendWrapper::new(api::status(&self.path))
            .await?
            .user
            .and_then(|mut user| user.tasks.remove(task).and_then(|task| task.current_input))
        {
            input
        } else {
            SendWrapper::new(api::generate_input(&self.path, task)).await?
        };
        let input_data = SendWrapper::new(api::get_file(&self.path, &input.path)).await?;
        let lang_ext = backend::languages()
            .iter()
            .find(|l| l.name == language)
            .map(|l| l.extensions[0].clone())
            .unwrap();

        let mut source = Vec::new();
        for (_name, content) in &files {
            source.extend_from_slice(content);
            if !content.ends_with(b"\n") {
                source.push(b'\n');
            }
        }

        let state: RwSignal<RunState> = expect_context();
        let (sender, mut receiver) = futures_channel::mpsc::unbounded();
        match &mut state.write().submit {
            StateSubmit::Submitting(proxy) => proxy.replace(Arc::new(move |msg| {
                let WorkerResponse::Execution(msg) = msg else {
                    unreachable!("Expected WorkerExecResponse");
                };
                sender
                    .unbounded_send(msg)
                    .expect("Failed to send message to submission proxy");
            })),
            _ => unreachable!("State should be in Submitting state when submit is called"),
        };

        backend::for_lang(language).send_message(
            WorkerExecRequest::Run {
                files: files
                    .into_iter()
                    .map(|(name, content)| File { name, content })
                    .collect(),
                primary_file: primary_file.to_string(),
                language: language.to_string(),
                input: Some(input_data),
                config: ExecConfig {
                    time_limit: Some(15.),
                    ..Default::default()
                },
            }
            .into(),
        );

        let mut output = Vec::new();
        let mut message = None;
        while let Some(msg) = receiver.next().await {
            match msg {
                WorkerExecResponse::StdoutChunk(chunk) => output.extend_from_slice(&chunk),
                WorkerExecResponse::Success => break,
                WorkerExecResponse::Error(err) => {
                    message = Some(err);
                    // Even if the execution fails, we still want to submit whatever output it produced
                    break;
                }
                WorkerExecResponse::Status(_)
                | WorkerExecResponse::CompilationMessageChunk(_)
                | WorkerExecResponse::StderrChunk(_) => {}
            }
        }

        let source_name = format!("{task}.{lang_ext}");
        let uploaded_source = SendWrapper::new(api::upload_source(
            &self.path,
            &input.id,
            &source_name,
            &source,
        ))
        .await?;

        let output_name = format!("{task}.output.txt");
        let uploaded_output = SendWrapper::new(api::upload_output(
            &self.path,
            &input.id,
            &output_name,
            &output,
        ))
        .await?;

        let sub = SendWrapper::new(api::submit(
            &self.path,
            &input.id,
            uploaded_output.id,
            uploaded_source.id,
        ))
        .await?;
        Ok(SubmitStatus {
            score: sub.score,
            message,
        })
    }
}

mod api {
    #![allow(dead_code)]

    use std::{collections::HashMap, ops::Range};

    use anyhow::{Result, anyhow, ensure};
    use chrono::{DateTime, Utc};
    use gloo_net::http::Request;
    use js_sys::Uint8Array;
    use serde::{Deserialize, Serialize};
    use web_sys::{Blob, FormData};

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum ValidationStatus {
        Missing,
        Parsed,
        Invalid,
    }

    #[derive(Debug, Deserialize)]
    pub struct ValidationCase {
        pub status: ValidationStatus,
        pub message: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum AlertSeverity {
        Warning,
        Danger,
        Success,
    }

    #[derive(Debug, Deserialize)]
    pub struct Alert {
        pub severity: AlertSeverity,
        pub message: String,
        pub blocking: bool,
    }

    #[derive(Debug, Deserialize)]
    pub struct Validation {
        pub alerts: Vec<Alert>,
        pub cases: Vec<ValidationCase>,
    }

    #[derive(Debug, Deserialize)]
    pub struct FeedbackCase {
        pub correct: bool,
        pub message: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Feedback {
        pub alerts: Vec<Alert>,
        pub cases: Vec<FeedbackCase>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Subtask {
        pub score: f64,
        pub max_score: f64,
        pub testcases: Vec<u32>,
        pub labels: Option<Vec<String>>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Task {
        pub name: String,
        pub title: String,
        pub statement_path: String,
        pub max_score: f64,
        pub num: i64,
        pub submission_timeout: Option<i64>,
    }

    #[derive(Debug, Deserialize)]
    pub struct ContestStatus {
        pub has_started: bool,
        pub time: Range<DateTime<Utc>>,
        pub name: String,
        pub description: String,
        //pub extra_material: Vec<ExtraMaterialSection>,
        pub tasks: Option<Vec<Task>>,
        pub max_total_score: Option<f64>,
    }

    #[derive(Debug, Deserialize)]
    pub struct UserTaskInfo {
        pub name: String,
        pub score: f64,
        pub current_input: Option<Input>,
    }

    #[derive(Debug, Deserialize)]
    pub struct UserStatus {
        pub tasks: HashMap<String, UserTaskInfo>,
        pub total_score: f64,
    }

    #[derive(Debug, Deserialize)]
    pub struct StatusResponse {
        pub user: Option<UserStatus>,
        pub contest: ContestStatus,
    }

    #[derive(Debug, Deserialize)]
    pub struct Input {
        pub id: String,
        pub token: String,
        pub task: String,
        pub attempt: i64,
        pub date: DateTime<Utc>,
        pub path: String,
        pub size: i64,
        pub expiry_date: Option<DateTime<Utc>>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Output {
        pub id: String,
        pub input: String,
        pub date: DateTime<Utc>,
        pub path: String,
        pub size: i64,
        pub validation: Validation,
    }

    #[derive(Debug, Deserialize)]
    pub struct Source {
        pub id: String,
        pub input: String,
        pub date: DateTime<Utc>,
        pub path: String,
        pub size: i64,
        pub validation: Validation,
    }

    #[derive(Debug, Serialize)]
    pub struct SubmitRequest {
        pub output_id: String,
        pub source_id: String,
    }

    #[derive(Debug, Deserialize)]
    pub struct Submission {
        pub id: String,
        pub token: String,
        pub task: String,
        pub score: f64,
        pub date: DateTime<Utc>,
        pub feedback: Feedback,
        pub input: Input,
        pub output: Output,
        pub source: Source,
        pub subtasks: Vec<Subtask>,
    }

    pub async fn status(path: &str) -> Result<StatusResponse> {
        let res = Request::get(&format!("{path}/api/status")).send().await?;
        ensure!(res.ok(), "Failed to get contest status: {}", res.status());
        let res = res.json().await?;
        Ok(res)
    }

    pub async fn get_statement_file(path: &str, task: &str, file: &str) -> Result<Vec<u8>> {
        let res = Request::get(&format!("{path}/statements/{task}/{file}"))
            .send()
            .await?;
        ensure!(res.ok(), "Failed to get statement file: {}", res.status());
        let res = res.binary().await?;
        Ok(res)
    }

    pub async fn get_file(path: &str, file: &str) -> Result<Vec<u8>> {
        let res = Request::get(&format!("{path}/files/{file}")).send().await?;
        ensure!(res.ok(), "Failed to get file: {}", res.status());
        let res = res.binary().await?;
        Ok(res)
    }

    pub async fn generate_input(path: &str, task: &str) -> Result<Input> {
        let res = Request::post(&format!("{path}/api/generate_input/{task}"))
            .send()
            .await?;
        ensure!(res.ok(), "Failed to generate input: {}", res.status());
        let res = res.json().await?;
        Ok(res)
    }

    pub async fn upload_output(
        path: &str,
        input_id: &str,
        file_name: &str,
        data: &[u8],
    ) -> Result<Output> {
        let array = js_sys::Array::of1(&Uint8Array::from(data));
        let blob = Blob::new_with_u8_array_sequence(&array)
            .map_err(|err| anyhow!("Failed to create blob: {err:?}"))?;
        let form = FormData::new().map_err(|err| anyhow!("Failed to create form data: {err:?}"))?;
        form.append_with_blob_and_filename("file", &blob, file_name)
            .map_err(|err| anyhow!("Failed to append file to form data: {err:?}"))?;

        let res = Request::post(&format!("{path}/api/upload_output/{input_id}"))
            .body(form)?
            .send()
            .await?;
        ensure!(res.ok(), "Failed to upload output: {}", res.status());
        let res = res.json().await?;
        Ok(res)
    }

    pub async fn upload_source(
        path: &str,
        input_id: &str,
        file_name: &str,
        data: &[u8],
    ) -> Result<Source> {
        let array = js_sys::Array::of1(&Uint8Array::from(data));
        let blob = Blob::new_with_u8_array_sequence(&array)
            .map_err(|err| anyhow!("Failed to create blob: {err:?}"))?;
        let form = FormData::new().map_err(|err| anyhow!("Failed to create form data: {err:?}"))?;
        form.append_with_blob_and_filename("file", &blob, file_name)
            .map_err(|err| anyhow!("Failed to append file to form data: {err:?}"))?;

        let res = Request::post(&format!("{path}/api/upload_source/{input_id}"))
            .body(form)?
            .send()
            .await?;
        ensure!(res.ok(), "Failed to upload source: {}", res.status());
        let res = res.json().await?;
        Ok(res)
    }

    pub async fn submit(
        path: &str,
        input_id: &str,
        output_id: String,
        source_id: String,
    ) -> Result<Submission> {
        let res = Request::post(&format!("{path}/api/submit/{input_id}"))
            .json(&SubmitRequest {
                output_id,
                source_id,
            })?
            .send()
            .await?;
        ensure!(res.ok(), "Failed to submit: {}", res.status());
        let res = res.json().await?;
        Ok(res)
    }
}
