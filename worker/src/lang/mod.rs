use anyhow::Result;
use common::Language;

use crate::os::FileEntry;

mod cpp;
mod python;

pub async fn run(language: Language, code: Vec<u8>, input: FileEntry) -> Result<()> {
    match language {
        Language::C => cpp::run(false, code, input).await,
        Language::CPP => cpp::run(true, code, input).await,
        Language::Python => python::run(code, input).await,
    }
}
