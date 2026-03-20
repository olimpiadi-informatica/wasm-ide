//! Global configuration for the application.

use std::collections::HashMap;

use serde::Deserialize;

/// Global configuration for the application.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Endpoint for the remote evaluation service. If `None`, remote evaluation is disabled.
    pub remote_eval: Option<String>,
    /// Files for newly created workspaces.
    pub default_ws: Ws,
    /// Size in bytes of compilers tarball.
    pub compilers: HashMap<String, u64>,
}

/// Files for newly created workspaces.
#[derive(Debug, Clone, Deserialize)]
pub struct Ws {
    /// Code files for the workspace.
    pub code: WsDir,
    /// Input files for the workspace.
    pub stdin: WsDir,
}

/// A mapping from file names to their content for a workspace.
pub type WsDir = HashMap<String, Content>;

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
