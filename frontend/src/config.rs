use std::collections::HashMap;

use common::config::WorkerConfig;
use serde::Deserialize;

use crate::{contest_api::ContestConfig, settings::StoredSettings};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub default_ws: Workspace,
    pub remote_eval: Option<String>,
    pub contest: Option<ContestConfig>,
    #[serde(default)]
    pub(crate) default_settings: StoredSettings,
    #[serde(flatten)]
    pub worker: WorkerConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Workspace {
    pub code: WorkspaceDir,
    pub stdin: WorkspaceDir,
}

pub type WorkspaceDir = HashMap<String, Content>;

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Content {
    String(String),
    Bytes(Vec<u8>),
}

impl Content {
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Content::String(s) => s.as_bytes(),
            Content::Bytes(b) => b,
        }
    }
}

impl From<String> for Content {
    fn from(s: String) -> Self {
        Content::String(s)
    }
}

impl From<Vec<u8>> for Content {
    fn from(b: Vec<u8>) -> Self {
        match String::from_utf8(b) {
            Ok(s) => Content::String(s),
            Err(e) => Content::Bytes(e.into_bytes()),
        }
    }
}
