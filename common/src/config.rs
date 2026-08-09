//! Configuration shared with the Web Worker.

use std::collections::HashMap;

use serde::Deserialize;

/// Configuration related to the Web Worker.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkerConfig {
    /// Size in bytes of compilers tarball.
    pub compilers: HashMap<String, u64>,
}
