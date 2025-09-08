use anyhow::{bail, Result};
use common::{File, Language};

use crate::os::Pipe;

mod cpp;
mod python;

pub async fn run(language: Language, files: Vec<File>, stdin: Pipe, stdout: Pipe) -> Result<()> {
    match language {
        Language::C => cpp::run(false, files, stdin, stdout).await,
        Language::CPP => cpp::run(true, files, stdin, stdout).await,
        Language::Python => python::run(files, stdin, stdout).await,
    }
}

pub async fn run_ls(language: Language, stdin: Pipe, stdout: Pipe, stderr: Pipe) -> Result<()> {
    match language {
        Language::C => cpp::run_ls(false, stdin, stdout, stderr).await,
        Language::CPP => cpp::run_ls(true, stdin, stdout, stderr).await,
        Language::Python => bail!("Language not supported for LS: {:?}", language),
    }
}
