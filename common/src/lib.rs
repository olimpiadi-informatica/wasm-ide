//! Types and utilities shared between the frontend and the worker.
//!
//! The Web Worker that executes and compiles user code communicates with the
//! browser frontend exclusively through the [`WorkerRequest`] and
//! [`WorkerResponse`] enums defined in this crate. The frontend serializes a
//! [`WorkerRequest`] and posts it to the worker, which replies with serialized
//! [`WorkerResponse`] values. Each message variant documents one step of the
//! compilation or language-server lifecycle that drives the IDE.
#![warn(missing_docs)]

use serde::{Deserialize, Serialize};
use tracing_subscriber::{fmt::format::Pretty, prelude::*};
use tracing_web::{performance_layer, MakeWebConsoleWriter};

/// Messages sent from the frontend to the worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkerRequest {
    /// Ask the worker to compile `source` in `language` and then run it.
    CompileAndRun {
        /// The user's source code to compile and run.
        source: String,
        /// Programming language of the source code.
        language: Language,
        /// Optional data written to the program's standard input before execution.
        input: Option<Vec<u8>>,
    },
    /// Additional chunk of data for the running program's standard input.
    StdinChunk(Vec<u8>),
    /// Cancel the current compilation or execution.
    Cancel,
    /// Start the language server for the given language.
    StartLS(Language),
    /// Forward a raw Language Server Protocol message to the worker.
    LSMessage(String),
}

/// Messages emitted by the worker back to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkerResponse {
    /// The worker finished initialization and is ready to receive messages.
    Ready,
    /// A chunk of bytes produced on the program's standard output.
    StdoutChunk(Vec<u8>),
    /// A chunk of bytes produced on the program's standard error.
    StderrChunk(Vec<u8>),
    /// A chunk of messages produced by the compiler while compiling the
    /// program.
    CompilationMessageChunk(Vec<u8>),
    /// An unrecoverable error occurred.
    Error(String),
    /// The language server finished starting and is ready.
    LSReady,
    /// The language server is shutting down.
    LSStopping,
    /// A Language Server Protocol message produced by the worker.
    LSMessage(String),
    /// Program execution finished.
    Done,
    /// The worker has begun processing a `CompileAndRun` request.
    Started,
    /// The compiler has been downloaded and is ready to use.
    CompilerFetched,
    /// Compilation has completed successfully.
    CompilationDone,
}

/// Languages supported by the IDE.
#[derive(PartialEq, Eq, Clone, Copy, Hash, Debug, Serialize, Deserialize)]
pub enum Language {
    /// C code.
    C,
    /// C++ code.
    CPP,
    /// Python 3 code.
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
