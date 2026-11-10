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

use std::{
    ops::Range,
    sync::{Arc, OnceLock},
};

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gloo_net::http::Response;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContestPhase {
    NotStarted,
    Running,
    Ended,
    /// The contest is not currently open and exact start/end timestamps are unavailable.
    NotOpen,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContestStatus {
    pub logged_in: bool,
    pub login_url: String,
    pub phase: ContestPhase,
    pub time: Option<Range<DateTime<Utc>>>,
    /// Difference `server_now_ms - client_now_ms` derived from the HTTP `Date` header, if present.
    pub clock_offset_ms: i64,
}

impl ContestStatus {
    pub fn effective_phase(&self, client_now_ms: i64) -> ContestPhase {
        let Some(time) = &self.time else {
            return self.phase;
        };
        let server_ms = client_now_ms.saturating_add(self.clock_offset_ms);
        let Some(now) = DateTime::from_timestamp_millis(server_ms) else {
            return self.phase;
        };
        if now < time.start {
            ContestPhase::NotStarted
        } else if now >= time.end {
            ContestPhase::Ended
        } else if self.phase == ContestPhase::NotStarted {
            ContestPhase::NotStarted
        } else {
            ContestPhase::Running
        }
    }

    pub fn countdown_seconds(&self, client_now_ms: i64) -> Option<i64> {
        let time = self.time.as_ref()?;
        let server_ms = client_now_ms.saturating_add(self.clock_offset_ms);
        let now = DateTime::from_timestamp_millis(server_ms)?;
        match self.effective_phase(client_now_ms) {
            ContestPhase::NotStarted if now < time.start => Some(
                ((time.start - now).num_milliseconds() + 999)
                    .div_euclid(1000)
                    .max(0),
            ),
            ContestPhase::Running if now < time.end => Some(
                ((time.end - now).num_milliseconds() + 999)
                    .div_euclid(1000)
                    .max(0),
            ),
            _ => None,
        }
    }
}

pub(crate) fn endpoint_login_url(endpoint: &str) -> String {
    let trimmed = endpoint.trim();
    if trimmed.is_empty() {
        "/".to_owned()
    } else {
        trimmed.to_owned()
    }
}

pub(crate) fn response_clock_offset_ms(res: &Response) -> i64 {
    let client_now = js_sys::Date::now();
    res.headers()
        .get("date")
        .map(|date| js_sys::Date::parse(&date))
        .filter(|ms| ms.is_finite())
        .map(|server_ms| (server_ms - client_now).round() as i64)
        .unwrap_or(0)
}

pub fn format_countdown(total_seconds: i64) -> String {
    let secs = total_seconds.max(0);
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    if hours >= 24 {
        let days = hours / 24;
        let rem_hours = hours % 24;
        format!("{days}d {rem_hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    }
}

/// Common interface between the frontend and a remote contest system.
///
/// Implementations translate the contest system's protocol into the task, workspace, scoring, and
/// submission model used by the UI. They are stored behind a shared [`DynContestAPI`], so an
/// implementation must also be `Send + Sync`.
#[async_trait]
pub trait ContestAPI {
    /// Queries authentication status and contest timing/phase information.
    async fn contest_status(&self) -> Result<ContestStatus>;

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

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::{ContestPhase, ContestStatus, format_countdown};

    #[wasm_bindgen_test]
    fn contest_status_computes_phases_and_ceil_countdown() {
        let start = DateTime::from_timestamp(1_000, 0).unwrap();
        let end = DateTime::from_timestamp(4_600, 0).unwrap();
        let status = ContestStatus {
            logged_in: true,
            login_url: "/".to_owned(),
            phase: ContestPhase::Running,
            time: Some(start..end),
            clock_offset_ms: 0,
        };

        assert_eq!(status.effective_phase(999_200), ContestPhase::NotStarted);
        assert_eq!(status.countdown_seconds(999_200), Some(1));

        assert_eq!(status.effective_phase(1_000_000), ContestPhase::Running);
        assert_eq!(status.countdown_seconds(1_000_000), Some(3_600));

        assert_eq!(status.effective_phase(4_600_000), ContestPhase::Ended);
        assert_eq!(status.countdown_seconds(4_600_000), None);
    }

    #[wasm_bindgen_test]
    fn countdown_formats_hours_and_days() {
        assert_eq!(format_countdown(65), "00:01:05");
        assert_eq!(format_countdown(3_661), "01:01:01");
        assert_eq!(format_countdown(90_061), "1d 01:01:01");
    }
}
