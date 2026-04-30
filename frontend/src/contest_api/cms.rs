use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use common::config::Workspace;
use gloo_timers::future::TimeoutFuture;
use send_wrapper::SendWrapper;

use crate::{
    backend,
    contest_api::{ContestAPI, SubmitStatus, Task},
};

pub struct Cms {
    #[allow(dead_code)]
    path: String,
}

impl Cms {
    pub fn new(path: String) -> Self {
        Self { path }
    }
}

#[async_trait]
impl ContestAPI for Cms {
    async fn list_tasks(&self) -> Result<Vec<Task>> {
        Ok(SendWrapper::new(api::task_list(&self.path))
            .await?
            .into_iter()
            .map(|task| Task {
                id: task.name.clone(),
                // TODO(virv): fetch the task title
                name: task.name,
            })
            .collect())
    }

    async fn init_workspace(&self, task: &str, lang: &str) -> Result<Workspace> {
        let lang_ext = backend::languages()
            .iter()
            .find(|l| l.name == lang)
            .map(|l| l.extensions[0].clone())
            .ok_or_else(|| anyhow!("Unsupported language: {lang}"))?;

        let source_name = format!("{task}.{lang_ext}");
        let source = SendWrapper::new(api::get_attachment(&self.path, task, &source_name)).await?;
        let mut code = common::config::WorkspaceDir::new();
        code.insert(source_name, source.into());
        if let Some(grader) =
            SendWrapper::new(api::get_optional_attachment(&self.path, task, "grader.cpp")).await?
        {
            code.insert("grader.cpp".to_string(), grader.into());
        }

        let mut stdin = common::config::WorkspaceDir::new();
        for i in 0.. {
            let input_name = format!("{task}.input{i}.txt");
            let input =
                SendWrapper::new(api::get_optional_attachment(&self.path, task, &input_name))
                    .await?;
            let Some(input) = input else {
                break;
            };
            stdin.insert(input_name, input.into());
        }

        Ok(Workspace { code, stdin })
    }

    async fn task_score(&self, _task: &str) -> Result<(f64, f64)> {
        let submissions = SendWrapper::new(api::submission_list(&self.path, _task)).await?;
        let Some(submission) = submissions.last() else {
            return Ok((0.0, 100.0));
        };
        let submission =
            SendWrapper::new(api::submission_info(&self.path, _task, &submission.id)).await?;
        Ok((submission.task_public_score.unwrap_or(0.0), 100.0))
    }

    async fn submit(
        &self,
        task: &str,
        language: &str,
        _primary_file: &str,
        files: Vec<(String, Vec<u8>)>,
    ) -> Result<SubmitStatus> {
        let lang_ext = backend::languages()
            .iter()
            .find(|l| l.name == language)
            .map(|l| l.extensions[0].clone())
            .ok_or_else(|| anyhow!("Unsupported language: {language}"))?;

        let task_info = SendWrapper::new(api::task_list(&self.path))
            .await?
            .into_iter()
            .find(|candidate| candidate.name == task)
            .ok_or_else(|| anyhow!("CMS task not found: {task}"))?;

        let files_by_name = files
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();
        let mut submission_files = Vec::new();
        for filename in task_info.submission_format {
            let workspace_name = filename.replace("%l", &lang_ext);
            let Some(content) = files_by_name.get(&workspace_name) else {
                bail!("Missing required CMS submission file: {workspace_name}");
            };
            submission_files.push((filename, workspace_name, content.clone()));
        }

        let submission_id =
            SendWrapper::new(api::submit(&self.path, task, submission_files)).await?;
        let score = loop {
            let submission =
                SendWrapper::new(api::submission_info(&self.path, task, &submission_id)).await?;
            if let Some(score) = submission.public_score {
                break score;
            }
            SendWrapper::new(TimeoutFuture::new(1000)).await;
        };

        Ok(SubmitStatus {
            score,
            message: None,
        })
    }
}

mod api {
    use anyhow::{Result, anyhow};
    use gloo_net::http::Request;
    use gloo_utils::document;
    use js_sys::Uint8Array;
    use serde::Deserialize;
    use wasm_bindgen::JsCast;
    use web_sys::{Blob, FormData, HtmlDocument};

    use crate::util::check_response;

    #[derive(Debug, Deserialize)]
    struct TaskListResponse {
        tasks: Vec<TaskListEntry>,
    }

    #[derive(Debug, Deserialize)]
    pub struct TaskListEntry {
        pub name: String,
        pub submission_format: Vec<String>,
    }

    #[derive(Debug, Deserialize)]
    struct SubmitResponse {
        id: String,
    }

    #[derive(Debug, Deserialize)]
    pub struct SubmissionListResponse {
        pub list: Vec<SubmissionListEntry>,
    }

    #[derive(Debug, Deserialize)]
    pub struct SubmissionListEntry {
        pub id: String,
    }

    #[derive(Debug, Deserialize)]
    pub struct SubmissionInfo {
        pub public_score: Option<f64>,
        pub task_public_score: Option<f64>,
    }

    pub async fn task_list(path: &str) -> Result<Vec<TaskListEntry>> {
        let res = Request::get(&format!("{path}/api/task_list"))
            .send()
            .await?;
        check_response(&res, "Failed to fetch CMS task list").await?;
        let res: TaskListResponse = res.json().await?;
        Ok(res.tasks)
    }

    pub async fn get_attachment(path: &str, task: &str, file: &str) -> Result<Vec<u8>> {
        let res = Request::get(&format!("{path}/tasks/{task}/attachments/{file}"))
            .send()
            .await?;
        check_response(&res, "Failed to get CMS attachment").await?;
        Ok(res.binary().await?)
    }

    pub async fn get_optional_attachment(
        path: &str,
        task: &str,
        file: &str,
    ) -> Result<Option<Vec<u8>>> {
        let res = Request::get(&format!("{path}/tasks/{task}/attachments/{file}"))
            .send()
            .await?;
        if res.status() == 404 {
            return Ok(None);
        }
        check_response(&res, "Failed to get CMS attachment").await?;
        Ok(Some(res.binary().await?))
    }

    pub async fn submit(
        path: &str,
        task: &str,
        files: Vec<(String, String, Vec<u8>)>,
    ) -> Result<String> {
        let form = FormData::new().map_err(|err| anyhow!("Failed to create form data: {err:?}"))?;
        for (field_name, file_name, data) in files {
            let array = js_sys::Array::of1(&Uint8Array::from(data.as_slice()));
            let blob = Blob::new_with_u8_array_sequence(&array)
                .map_err(|err| anyhow!("Failed to create blob: {err:?}"))?;
            form.append_with_blob_and_filename(&field_name, &blob, &file_name)
                .map_err(|err| anyhow!("Failed to append file to form data: {err:?}"))?;
        }

        let xsrf = get_xsrf_cookie()?;
        let res = Request::post(&format!("{path}/api/{task}/submit"))
            .header("X-XSRFToken", &xsrf)
            .body(form)?
            .send()
            .await?;
        check_response(&res, "Failed to submit to CMS").await?;
        let res: SubmitResponse = res.json().await?;
        Ok(res.id)
    }

    pub async fn submission_list(path: &str, task: &str) -> Result<Vec<SubmissionListEntry>> {
        let res = Request::get(&format!("{path}/api/{task}/submission_list"))
            .send()
            .await?;
        check_response(&res, "Failed to fetch CMS submission list").await?;
        let res: SubmissionListResponse = res.json().await?;
        Ok(res.list)
    }

    pub async fn submission_info(path: &str, task: &str, id: &str) -> Result<SubmissionInfo> {
        let res = Request::get(&format!("{path}/tasks/{task}/submissions/{id}"))
            .send()
            .await?;
        check_response(&res, "Failed to fetch CMS submission info").await?;
        let res = res.json().await?;
        Ok(res)
    }

    fn get_xsrf_cookie() -> Result<String> {
        let cookie = document()
            .dyn_into::<HtmlDocument>()
            .map_err(|_| anyhow!("Document is not an HtmlDocument"))?
            .cookie()
            .map_err(|err| anyhow!("Failed to read cookies: {err:?}"))?;
        cookie
            .split("; ")
            .find_map(|entry: &str| entry.strip_prefix("_xsrf="))
            .map(ToString::to_string)
            .ok_or_else(|| anyhow!("CMS XSRF cookie is missing"))
    }
}
