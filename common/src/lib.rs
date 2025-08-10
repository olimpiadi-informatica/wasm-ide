use serde::{Deserialize, Serialize};
use tracing_subscriber::{fmt::format::Pretty, prelude::*};
use tracing_web::{performance_layer, MakeWebConsoleWriter};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessage {
    CompileAndRun {
        source: String,
        language: Language,
        input: Option<Vec<u8>>,
    },
    StdinChunk(Vec<u8>),
    Cancel,
    StartLS(Language),
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

pub fn init_logging() {
    console_error_panic_hook::set_once();

    let filter_layer = tracing_subscriber::filter::Targets::new()
        .with_target("common", tracing::Level::TRACE)
        .with_target("worker", tracing::Level::TRACE)
        .with_target("frontend", tracing::Level::TRACE)
        .with_default(tracing::Level::INFO);

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false) // Only partially supported across browsers
        .without_time() // std::time is not available in browsers, see note below
        .with_writer(MakeWebConsoleWriter::new()); // write events to the console
    let perf_layer = performance_layer().with_details_from_fields(Pretty::default());

    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(perf_layer)
        .with(filter_layer)
        .init(); // Install these as subscribers to tracing events
}
