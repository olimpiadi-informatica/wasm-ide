//! Types and utilities shared between the frontend and the worker.
//!
//! The Web Worker that executes and compiles user code communicates with the
//! browser frontend exclusively through the [`WorkerRequest`] and
//! [`WorkerResponse`] enums defined in this crate. The frontend serializes a
//! [`WorkerRequest`] and posts it to the worker, which replies with serialized
//! [`WorkerResponse`] values. Each message variant documents one step of the
//! compilation or language-server lifecycle that drives the IDE.
#![warn(missing_docs)]

use std::fmt::Display;

use derive_more::From;
use serde::{Deserialize, Serialize};
use strum::VariantArray;
use tracing_subscriber::fmt::format::Pretty;
use tracing_subscriber::prelude::*;
use tracing_web::{performance_layer, MakeWebConsoleWriter};

/// Messages sent from the frontend to the worker.
#[derive(Debug, Serialize, Deserialize, From)]
pub enum WorkerRequest {
    /// Control program execution.
    Execution(#[from] WorkerExecRequest),
    /// Control the language server.
    LS(#[from] WorkerLSRequest),
}

/// Messages emitted by the worker back to the frontend.
#[derive(Debug, Serialize, Deserialize, From)]
pub enum WorkerResponse {
    /// The worker finished initialization and is ready to receive messages.
    Ready,

    /// A message related to program execution.
    Execution(#[from] WorkerExecResponse),
    /// A message related to the language server.
    LS(#[from] WorkerLSResponse),

    /// The worker is downloading the compiler, with an optional progress
    /// report (bytes downloaded, total bytes).
    FetchingCompiler(String, Option<(u64, u64)>),
    /// The worker has finished downloading the compiler.
    CompilerFetchDone(String),
}

/// Messages sent from the frontend to the worker to control program execution.
#[derive(Debug, Serialize, Deserialize)]
pub enum WorkerExecRequest {
    /// Ask the worker to compile `source` in `language` and then run it.
    CompileAndRun {
        /// The user's source code to compile and run.
        files: Vec<File>,
        /// Programming language of the source code.
        language: Language,
        /// Optional data written to the program's standard input before execution.
        input: Option<Vec<u8>>,
        /// Configuration for program execution.
        config: ExecConfig,
    },
    /// Additional chunk of data for the running program's standard input.
    StdinChunk(Vec<u8>),
    /// Cancel the current compilation or execution.
    Cancel,
}

/// Configuration for program execution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecConfig {
    /// Optional maximum memory (in 64KB pages) the program is allowed to use.
    pub mem_limit: Option<u32>,
}

/// Messages emitted by the worker back to the frontend to report on program
/// execution.
#[derive(Debug, Serialize, Deserialize)]
pub enum WorkerExecResponse {
    /// The current status of program execution.
    Status(WorkerExecStatus),

    /// A chunk of messages produced by the compiler while compiling the program.
    CompilationMessageChunk(Vec<u8>),
    /// A chunk of bytes produced on the program's standard output.
    StdoutChunk(Vec<u8>),
    /// A chunk of bytes produced on the program's standard error.
    StderrChunk(Vec<u8>),

    /// The program finished execution with an error.
    Error(String),
    /// The program finished execution successfully.
    Success,
}

/// Messages sent from the frontend to the worker to control the language server.
#[derive(Debug, Serialize, Deserialize)]
pub enum WorkerLSRequest {
    /// Start the language server for the given language.
    Start(Language),
    /// Forward a raw Language Server Protocol message to the worker.
    Message(String),
}

/// Messages emitted by the worker back to the frontend to report on the
/// language server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkerLSResponse {
    /// The language server is downloading the compiler.
    FetchingCompiler,
    /// The language server finished starting and is ready.
    Started,
    /// A Language Server Protocol message produced by the worker.
    Message(String),
    /// The language server is shutting down.
    Stopped,
    /// The language server encountered an error.
    Error(String),
}

/// The current status of program execution in the worker.
///
/// Each status corresponds to a phase in the program execution lifecycle.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WorkerExecStatus {
    /// The worker is downloading the compiler.
    FetchingCompiler,
    /// The program is being compiled.
    Compiling,
    /// The program is currently running.
    Running,
}

/// A source code file.
#[derive(Debug, Serialize, Deserialize)]
pub struct File {
    /// The file's name.
    pub name: String,
    /// The file's content.
    pub content: String,
}

/// Languages supported by the IDE.
#[derive(PartialEq, Eq, Clone, Copy, Hash, Debug, Serialize, Deserialize, VariantArray)]
pub enum Language {
    /// C code.
    C,
    /// C++ code.
    CPP,
    /// Python 3 code.
    Python,
}

impl Language {
    /// Return the typical file extension for this language.
    pub fn ext(self) -> &'static str {
        match self {
            Language::C => "c",
            Language::CPP => "cpp",
            Language::Python => "py",
        }
    }
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

impl Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let l: &str = (*self).into();
        write!(f, "{l}")
    }
}

/// Initialize logging to the browser console.
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
