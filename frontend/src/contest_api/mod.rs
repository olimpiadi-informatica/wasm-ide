//! Contest-system integrations.
//!
//! At startup, [`init`] selects at most one configured contest system and stores it as a
//! [`DynContestAPI`]. The rest of the frontend uses [`get`] and the [`ContestAPI`] trait, so it
//! does not need to know which system is active.
//!
//! # Adding a contest system
//!
//! 1. Add a module alongside `cms` and `terry`. Keep protocol-specific request and response
//!    types inside that module.
//! 2. Add a variant containing the endpoint (or other required settings) to [`ContestConfig`].
//!    The enum is stored in the `contest` object in `config.json`; its `type` field selects the
//!    lowercase variant name.
//! 3. Implement [`ContestAPI`] for the new client. [`Task::id`] must be the stable identifier used
//!    by the remote system, while [`Task::name`] is the label shown to the user. Build a complete
//!    [`Workspace`] in `init_workspace`, return `(current, maximum)` from `task_score`, and return
//!    the resulting score and optional user-facing message from `submit`.
//! 4. Register the module below and add its constructor to the match in [`init`]. The enum ensures
//!    that at most one contest system can be configured.
//!
//! Browser-backed request futures may not implement `Send`, while `async_trait` requires `Send`
//! futures by default. Follow the existing integrations and wrap those futures with
//! `send_wrapper::SendWrapper` when necessary.

use std::sync::{Arc, OnceLock};

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;

use crate::config::Workspace;

mod cms;
mod terry;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ContestConfig {
    Terry { endpoint: String },
    Cms { endpoint: String },
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct SubmitStatus {
    pub score: f64,
    pub message: Option<String>,
}

/// Common interface between the frontend and a remote contest system.
///
/// Implementations translate the contest system's protocol into the task, workspace, scoring, and
/// submission model used by the UI. They are stored behind a shared [`DynContestAPI`], so an
/// implementation must also be `Send + Sync`.
#[async_trait]
pub trait ContestAPI {
    /// Lists the tasks available to the current user.
    ///
    /// Each returned [`Task`] must contain the stable remote identifier used by the other methods
    /// and the display name shown in the workspace selector.
    async fn list_tasks(&self) -> Result<Vec<Task>>;

    /// Creates the initial workspace contents for a task and programming language.
    ///
    /// `task` is an identifier returned by [`ContestAPI::list_tasks`], and `lang` is a language
    /// name exposed by the execution backend. The returned [`Workspace`] should contain every code
    /// and input file that the user needs to start working on the task.
    async fn init_workspace(&self, task: &str, lang: &str) -> Result<Workspace>;

    /// Returns the current and maximum score for a task, in that order.
    ///
    /// `task` is an identifier returned by [`ContestAPI::list_tasks`].
    async fn task_score(&self, task: &str) -> Result<(f64, f64)>;

    /// Submits the current workspace and waits until a result is available.
    ///
    /// `task` is the remote task identifier, `language` is the selected backend language,
    /// `primary_file` identifies the editor's primary source file, and `files` contains the name
    /// and raw contents of every code file in the workspace. The returned [`SubmitStatus`] contains
    /// the score for this submission and an optional message suitable for display to the user.
    async fn submit(
        &self,
        task: &str,
        language: &str,
        primary_file: &str,
        files: Vec<(String, Vec<u8>)>,
    ) -> Result<SubmitStatus>;
}

pub type DynContestAPI = Arc<dyn ContestAPI + Send + Sync>;

static SINGLETON: OnceLock<Option<DynContestAPI>> = OnceLock::new();

pub fn get() -> Option<DynContestAPI> {
    SINGLETON.get().expect("ContestAPI not initialized").clone()
}

pub async fn init(config: Option<&ContestConfig>) {
    let api: Option<DynContestAPI> = match config {
        Some(ContestConfig::Cms { endpoint }) => Some(Arc::new(cms::Cms::new(endpoint.clone()))),
        Some(ContestConfig::Terry { endpoint }) => {
            Some(Arc::new(terry::Terry::new(endpoint.clone())))
        }
        None => None,
    };
    SINGLETON
        .set(api)
        .ok()
        .expect("ContestAPI already initialized");
}
