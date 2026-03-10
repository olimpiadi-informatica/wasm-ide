use anyhow::Result;
use common::{ExecConfig, File, Language};

use crate::os::Pipe;

mod cpp;
mod python;

pub async fn run(
    language: Language,
    config: ExecConfig,
    files: Vec<File>,
    primary_file: String,
    stdin: Pipe,
    stdout: Pipe,
) -> Result<()> {
    match language {
        Language::C => cpp::run(config, files, stdin, stdout).await,
        Language::Cpp => cpp::run(config, files, stdin, stdout).await,
        Language::Python => python::run(config, files, primary_file, stdin, stdout).await,
    }
}

pub async fn run_ls(language: Language, stdin: Pipe, stdout: Pipe, stderr: Pipe) -> Result<()> {
    match language {
        Language::C => cpp::run_ls(false, stdin, stdout, stderr).await,
        Language::Cpp => cpp::run_ls(true, stdin, stdout, stderr).await,
        Language::Python => python::run_ls(stdin, stdout, stderr).await,
    }
}
