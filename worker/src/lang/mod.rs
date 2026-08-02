use std::ops::Deref;
use std::rc::Rc;

use anyhow::{Result, bail};
use common::{ExecConfig, File, Language};

use crate::os::{Fs, Pipe};

mod cpp;
mod python;
mod rust;

fn mirror_workdir(fs: &mut Fs, files: Vec<File>) {
    for file in files {
        fs.add_file_with_path(
            format!("/workdir/{}", file.name).as_bytes(),
            Rc::new(file.content),
        );
    }
}

pub async fn run(
    language: String,
    config: ExecConfig,
    files: Vec<File>,
    primary_file: String,
    stdin: Pipe,
    stdout: Pipe,
) -> Result<()> {
    match language.deref() {
        "C" => cpp::run(config, files, stdin, stdout).await,
        "C++" => cpp::run(config, files, stdin, stdout).await,
        "Python3" => python::run(config, files, primary_file, stdin, stdout).await,
        "Rust" => rust::run(config, files, primary_file, stdin, stdout).await,
        _ => bail!("Unsupported language: {}", language),
    }
}

pub async fn run_ls(
    language: String,
    files: Vec<File>,
    stdin: Pipe,
    stdout: Pipe,
    stderr: Pipe,
) -> Result<()> {
    match language.deref() {
        "C" => cpp::run_ls(false, files, stdin, stdout, stderr).await,
        "C++" => cpp::run_ls(true, files, stdin, stdout, stderr).await,
        "Python3" => python::run_ls(files, stdin, stdout, stderr).await,
        "Rust" => rust::run_ls(files, stdin, stdout, stderr).await,
        _ => bail!("Unsupported language: {}", language),
    }
}

pub fn list() -> Vec<Language> {
    vec![
        Language {
            name: "C++".to_string(),
            extensions: vec!["cpp".to_string(), "cc".to_string(), "c++".to_string()],
        },
        Language {
            name: "C".to_string(),
            extensions: vec!["c".to_string()],
        },
        Language {
            name: "Python3".to_string(),
            extensions: vec!["py".to_string()],
        },
        Language {
            name: "Rust".to_string(),
            extensions: vec!["rs".to_string()],
        },
    ]
}
