use std::sync::{Arc, OnceLock};

use anyhow::Result;
use async_trait::async_trait;
use common::config::{Config, Workspace};

mod terry;

#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct SubmitStatus {
    pub score: f64,
}

#[async_trait]
pub trait ContestAPI {
    async fn list_tasks(&self) -> Result<Vec<Task>>;

    async fn init_workspace(&self, task: &str, lang: &str) -> Result<Workspace>;

    async fn submit(
        &self,
        task: &str,
        language: &str,
        primary_file: &str,
        files: Vec<(String, String)>,
    ) -> Result<SubmitStatus>;
}

pub type DynContestAPI = Arc<dyn ContestAPI + Send + Sync>;

static SINGLETON: OnceLock<Option<DynContestAPI>> = OnceLock::new();

pub fn get() -> Option<DynContestAPI> {
    SINGLETON.get().expect("ContestAPI not initialized").clone()
}

pub async fn init(config: &Config) {
    let api: Option<DynContestAPI> = if let Some(terry) = config.terry.clone() {
        Some(Arc::new(terry::Terry::new(terry)))
    } else {
        None
    };
    SINGLETON
        .set(api)
        .ok()
        .expect("ContestAPI already initialized");
}
