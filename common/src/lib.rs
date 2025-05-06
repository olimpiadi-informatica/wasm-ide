use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessage {
    CompileAndRun {
        source: String,
        language: Language,
        input: Option<Vec<u8>>,
        base_url: String,
    },
    StdinChunk(Vec<u8>),
    Cancel,
    StartLS(String, Language),
    LSMessage(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkerMessage {
    Ready,
    StdoutChunk(Vec<u8>),
    StderrChunk(Vec<u8>),
    CompilationMessageChunk(Vec<u8>),
    Error(String),
    LSReady,
    LSStopping,
    LSMessage(String),
    Done,
    Started,
    CompilerFetched,
    CompilationDone,
}

#[derive(PartialEq, Eq, Clone, Copy, Hash, Debug, Serialize, Deserialize)]
pub enum Language {
    C,
    CPP,
    Python,
}

impl From<Language> for &'static str {
    fn from(val: Language) -> Self {
        match val {
            Language::C => "C",
            Language::CPP => "C++",
            Language::Python => "Python",
        }
    }
}

impl From<Language> for String {
    fn from(val: Language) -> Self {
        Into::<&'static str>::into(val).to_owned()
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Hash, Debug, Serialize, Deserialize)]
pub enum KeyboardMode {
    Standard,
    Vim,
    Emacs,
}

#[derive(PartialEq, Eq, Clone, Copy, Hash, Debug, Serialize, Deserialize)]
pub enum InputMode {
    Batch,
    MixedInteractive,
    FullInteractive,
}
