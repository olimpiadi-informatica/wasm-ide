use anyhow::Result;
use async_trait::async_trait;
use common::config::Workspace;
use send_wrapper::SendWrapper;

use crate::{
    backend,
    contest_api::{ContestAPI, Task},
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
}

mod api {
    #![allow(dead_code)]

    use anyhow::{Result, ensure};
    use gloo_net::http::Request;
    use serde::Deserialize;

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
        //pub time: Range<DateTime<Utc>>,
        pub name: String,
        pub description: String,
        //pub extra_material: Vec<ExtraMaterialSection>,
        pub tasks: Option<Vec<Task>>,
        pub max_total_score: Option<f64>,
    }

    #[derive(Debug, Deserialize)]
    pub struct StatusResponse {
        //pub user: Option<UserStatus>,
        pub contest: ContestStatus,
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
}
