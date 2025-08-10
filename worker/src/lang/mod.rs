use std::rc::Rc;

use anyhow::{bail, Result};
use common::Language;

use crate::os::{FdEntry, Pipe};

mod cpp;
mod python;

pub async fn run(language: Language, code: Vec<u8>, input: FdEntry) -> Result<()> {
    match language {
        Language::C => cpp::run(false, code, input).await,
        Language::CPP => cpp::run(true, code, input).await,
        Language::Python => python::run(code, input).await,
    }
}

pub async fn run_ls(
    language: Language,
    stdin: Rc<Pipe>,
    stdout: Rc<Pipe>,
    stderr: Rc<Pipe>,
) -> Result<()> {
    match language {
        Language::C => cpp::run_ls(false, stdin, stdout, stderr).await,
        Language::CPP => cpp::run_ls(true, stdin, stdout, stderr).await,
        Language::Python => bail!("Language not supported for LS: {:?}", language),
    }
}
