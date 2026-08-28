use std::collections::HashMap;

use common::config::WorkerConfig;
use serde::Deserialize;

use crate::{contest_api::ContestConfig, i18n::Locale, settings::StoredSettings};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub default_ws: Workspace,
    #[serde(default = "workspace_enabled_default")]
    pub workspace_enabled: bool,
    pub default_locale: Option<Locale>,
    pub remote_eval: Option<String>,
    pub contest: Option<ContestConfig>,
    #[serde(default)]
    pub(crate) default_settings: StoredSettings,
    #[serde(flatten)]
    pub worker: WorkerConfig,
}

impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.workspace_enabled || self.contest.is_none(),
            "contest integration requires workspaces to be enabled"
        );
        Ok(())
    }
}

const fn workspace_enabled_default() -> bool {
    true
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

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::Config;
    use crate::contest_api::ContestConfig;
    use crate::settings::StoredSettings;

    #[wasm_bindgen_test]
    fn config_defaults() {
        let config: Config = serde_json::from_value(json!({
            "default_ws": { "code": {}, "stdin": {} },
            "compilers": {}
        }))
        .unwrap();

        assert!(config.workspace_enabled);
        assert!(config.default_locale.is_none());
        assert!(config.remote_eval.is_none());
        assert!(config.contest.is_none());
        assert_eq!(config.default_settings, StoredSettings::default());
        assert!(config.worker.compilers.is_empty());
        config.validate().unwrap();
    }

    #[wasm_bindgen_test]
    fn config_uses_snake_case_settings_and_an_internally_tagged_contest() {
        let config: Config = serde_json::from_value(json!({
            "default_ws": { "code": {}, "stdin": {} },
            "default_settings": {
                "theme": "dark",
                "keyboard_mode": "vim",
                "input_mode": "mixed_interactive"
            },
            "contest": {
                "type": "terry",
                "endpoint": "https://example.invalid"
            },
            "compilers": {}
        }))
        .unwrap();

        let settings = serde_json::to_value(config.default_settings).unwrap();
        assert_eq!(settings["theme"], "dark");
        assert_eq!(settings["keyboard_mode"], "vim");
        assert_eq!(settings["input_mode"], "mixed_interactive");
        assert!(matches!(
            config.contest,
            Some(ContestConfig::Terry { ref endpoint }) if endpoint == "https://example.invalid"
        ));
        config.validate().unwrap();
    }

    #[wasm_bindgen_test]
    fn contest_requires_workspaces() {
        let config: Config = serde_json::from_value(json!({
            "default_ws": { "code": {}, "stdin": {} },
            "workspace_enabled": false,
            "contest": {
                "type": "cms",
                "endpoint": "https://example.invalid"
            },
            "compilers": {}
        }))
        .unwrap();

        assert_eq!(
            config.validate().unwrap_err().to_string(),
            "contest integration requires workspaces to be enabled"
        );
    }
}
