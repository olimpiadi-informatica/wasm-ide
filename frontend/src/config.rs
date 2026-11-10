use std::collections::HashMap;

use common::config::WorkerConfig;
use serde::Deserialize;

use crate::{contest_api::ContestConfig, i18n::Locale, settings::StoredSettings};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub default_templates: HashMap<String, WorkspaceDir>,
    #[serde(default)]
    pub default_stdin: WorkspaceDir,
    #[serde(default)]
    pub default_language: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    pub default_locale: Option<Locale>,
    pub remote_eval: Option<String>,
    pub contest: Option<ContestConfig>,
    #[serde(default = "auto_init_contest_tasks_default")]
    pub auto_init_contest_tasks: bool,
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
        if let Some(lang) = &self.default_language {
            anyhow::ensure!(!lang.trim().is_empty(), "default_language cannot be empty");
        }
        if let Some(lang) = &self.default_settings.default_language {
            anyhow::ensure!(
                !lang.trim().is_empty(),
                "default_settings.default_language cannot be empty"
            );
        }
        anyhow::ensure!(
            (1..=16).contains(&self.default_settings.tab_size),
            "default_settings.tab_size must be between 1 and 16"
        );
        Ok(())
    }

    pub fn local_workspace(&self, language: &str, primary_ext: &str) -> Workspace {
        let code = self
            .default_templates
            .get(language)
            .filter(|dir| !dir.is_empty())
            .cloned()
            .unwrap_or_else(|| {
                [(format!("main.{primary_ext}"), String::new().into())]
                    .into_iter()
                    .collect()
            });
        let stdin = if self.default_stdin.is_empty() {
            [("input.txt".to_owned(), String::new().into())]
                .into_iter()
                .collect()
        } else {
            self.default_stdin.clone()
        };
        Workspace { code, stdin }
    }
}

const fn auto_init_contest_tasks_default() -> bool {
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
            "compilers": {}
        }))
        .unwrap();

        assert!(config.default_templates.is_empty());
        assert!(config.default_stdin.is_empty());
        assert!(config.default_language.is_none());
        assert!(config.title.is_none());
        assert!(config.auto_init_contest_tasks);
        assert!(config.default_locale.is_none());
        assert!(config.remote_eval.is_none());
        assert!(config.contest.is_none());
        assert_eq!(config.default_settings, StoredSettings::default());
        assert!(config.worker.compilers.is_empty());
        config.validate().unwrap();
    }

    #[wasm_bindgen_test]
    fn config_default_language_and_auto_init() {
        let config: Config = serde_json::from_value(json!({
            "default_language": "C++",
            "auto_init_contest_tasks": false,
            "contest": {
                "type": "cms",
                "endpoint": "https://example.invalid"
            },
            "compilers": {}
        }))
        .unwrap();

        assert_eq!(config.default_language.as_deref(), Some("C++"));
        assert!(!config.auto_init_contest_tasks);
        config.validate().unwrap();
    }

    #[wasm_bindgen_test]
    fn title_rejects_empty() {
        let config: Config = serde_json::from_value(json!({
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
    fn default_language_rejects_empty() {
        let config: Config = serde_json::from_value(json!({
            "default_language": "   ",
            "compilers": {}
        }))
        .unwrap();

        assert_eq!(
            config.validate().unwrap_err().to_string(),
            "default_language cannot be empty"
        );
    }

    #[wasm_bindgen_test]
    fn config_uses_snake_case_settings_and_an_internally_tagged_contest() {
        let config: Config = serde_json::from_value(json!({
            "default_settings": {
                "theme": "dark",
                "keyboard_mode": "vim",
                "input_mode": "mixed_interactive",
                "tab_size": 2
            },
            "contest": {
                "type": "terry",
                "endpoint": "https://example.invalid"
            },
            "compilers": {}
        }))
        .unwrap();

        let settings = serde_json::to_value(&config.default_settings).unwrap();
        assert_eq!(settings["theme"], "dark");
        assert_eq!(settings["keyboard_mode"], "vim");
        assert_eq!(settings["input_mode"], "mixed_interactive");
        assert_eq!(settings["tab_size"], 2);
        assert!(matches!(
            config.contest,
            Some(ContestConfig::Terry { ref endpoint }) if endpoint == "https://example.invalid"
        ));
        config.validate().unwrap();
    }

    #[wasm_bindgen_test]
    fn local_workspace_uses_language_template_or_fallback() {
        let config: Config = serde_json::from_value(json!({
            "default_templates": {
                "C++": {
                    "main.cpp": "int main() {}\n"
                }
            },
            "default_stdin": {
                "input.txt": "42\n"
            },
            "compilers": {}
        }))
        .unwrap();

        let cpp_ws = config.local_workspace("C++", "cpp");
        assert_eq!(cpp_ws.code["main.cpp"].as_bytes(), b"int main() {}\n");
        assert_eq!(cpp_ws.stdin["input.txt"].as_bytes(), b"42\n");

        let py_ws = config.local_workspace("Python3", "py");
        assert_eq!(py_ws.code["main.py"].as_bytes(), b"");
        assert_eq!(py_ws.stdin["input.txt"].as_bytes(), b"42\n");
    }
}
