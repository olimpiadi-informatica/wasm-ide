//! Global configuration for the application.

use std::collections::HashMap;

use serde::Deserialize;

/// Global configuration for the application.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Files for newly created workspaces.
    pub default_ws: Workspace,
    /// Endpoint for the remote evaluation service. If `None`, remote evaluation is disabled.
    pub remote_eval: Option<String>,
    /// Endpoint for the Terry contest API. If `None`, Terry integration is disabled.
    pub terry: Option<String>,
    /// Endpoint for the CMS contest API. If `None`, CMS integration is disabled.
    pub cms: Option<String>,
    /// Size in bytes of compilers tarball.
    pub compilers: HashMap<String, u64>,
}

/// Files for newly created workspaces.
#[derive(Debug, Clone, Deserialize)]
pub struct Workspace {
    /// Code files for the workspace.
    pub code: WorkspaceDir,
    /// Input files for the workspace.
    pub stdin: WorkspaceDir,
}

/// A mapping from file names to their content for a workspace.
pub type WorkspaceDir = HashMap<String, Content>;

/// A type that can be encoded as a string or bytes content, used for json deserialization of
/// program input and output.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Content {
    /// A string content
    String(String),
    /// A bytes content
    Bytes(Vec<u8>),
}

impl Content {
    /// Convert the content to bytes, encoding strings as UTF-8.
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
