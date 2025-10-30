use anyhow::Result;
use common::{ExecConfig, File, Language};
use tracing::warn;

use crate::os::Pipe;

mod cpp;
mod python;

pub async fn run(
    language: Language,
    config: ExecConfig,
    files: Vec<File>,
    stdin: Pipe,
    stdout: Pipe,
) -> Result<()> {
    match language {
        Language::C => cpp::run(false, config, files, stdin, stdout).await,
        Language::CPP => cpp::run(true, config, files, stdin, stdout).await,
        Language::Python => python::run(config, files, stdin, stdout).await,
    }
}

pub async fn run_ls(language: Language, stdin: Pipe, stdout: Pipe, stderr: Pipe) -> Result<()> {
    match language {
        Language::C => cpp::run_ls(false, stdin, stdout, stderr).await,
        Language::CPP => cpp::run_ls(true, stdin, stdout, stderr).await,
        _ => {
            warn!("Language not supported for LS: {:?}", language);
            Ok(())
        }
    }
}
