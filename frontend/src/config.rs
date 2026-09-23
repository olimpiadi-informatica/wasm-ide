use std::collections::HashMap;

use common::config::WorkerConfig;
use serde::Deserialize;

use crate::{contest_api::ContestConfig, i18n::Locale, settings::StoredSettings};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub default_ws: Workspace,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default = "workspace_enabled_default")]
    pub workspace_enabled: bool,
    pub default_locale: Option<Locale>,
    pub remote_eval: Option<String>,
    pub contest: Option<ContestConfig>,
    #[serde(default)]
    pub auto_init_contest_tasks: Option<String>,
    #[serde(default)]
    pub(crate) default_settings: StoredSettings,
    #[serde(flatten)]
    pub worker: WorkerConfig,
}

impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        if let Some(title) = &self.title {
            anyhow::ensure!(!title.trim().is_empty(), "title cannot be empty");
        }
        anyhow::ensure!(
            self.workspace_enabled || self.contest.is_none(),
            "contest integration requires workspaces to be enabled"
        );
        if let Some(lang) = &self.auto_init_contest_tasks {
            anyhow::ensure!(
                self.contest.is_some(),
                "auto_init_contest_tasks requires a contest integration"
            );
            anyhow::ensure!(
                !lang.trim().is_empty(),
                "auto_init_contest_tasks language cannot be empty"
            );
        }
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
        assert!(config.title.is_none());
        assert!(config.auto_init_contest_tasks.is_none());
        assert!(config.default_locale.is_none());
        assert!(config.remote_eval.is_none());
        assert!(config.contest.is_none());
        assert_eq!(config.default_settings, StoredSettings::default());
        assert!(config.worker.compilers.is_empty());
        config.validate().unwrap();
    }

    #[wasm_bindgen_test]
    fn config_auto_init_contest_tasks() {
        let config: Config = serde_json::from_value(json!({
            "default_ws": { "code": {}, "stdin": {} },
            "auto_init_contest_tasks": "C++",
            "contest": {
                "type": "cms",
                "endpoint": "https://example.invalid"
            },
            "compilers": {}
        }))
        .unwrap();

        assert_eq!(config.auto_init_contest_tasks.as_deref(), Some("C++"));
        config.validate().unwrap();
    }

    #[wasm_bindgen_test]
    fn auto_init_requires_contest() {
        let config: Config = serde_json::from_value(json!({
            "default_ws": { "code": {}, "stdin": {} },
            "auto_init_contest_tasks": "C++",
            "compilers": {}
        }))
        .unwrap();

        assert_eq!(
            config.validate().unwrap_err().to_string(),
            "auto_init_contest_tasks requires a contest integration"
        );
    }

    #[wasm_bindgen_test]
    fn title_rejects_empty() {
        let config: Config = serde_json::from_value(json!({
            "default_ws": { "code": {}, "stdin": {} },
            "title": "   ",
            "compilers": {}
        }))
        .unwrap();

        assert_eq!(
            config.validate().unwrap_err().to_string(),
            "title cannot be empty"
        );
    }

    #[wasm_bindgen_test]
    fn auto_init_rejects_empty_language() {
        let config: Config = serde_json::from_value(json!({
            "default_ws": { "code": {}, "stdin": {} },
            "auto_init_contest_tasks": "   ",
            "contest": {
                "type": "cms",
                "endpoint": "https://example.invalid"
            },
            "compilers": {}
        }))
        .unwrap();

        assert_eq!(
            config.validate().unwrap_err().to_string(),
            "auto_init_contest_tasks language cannot be empty"
        );
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
