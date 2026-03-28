use std::cell::RefCell;

use common::{ExecConfig, File, WorkerExecRequest, WorkerExecResponse};
use futures::channel::oneshot::{Sender, channel};
use futures::{FutureExt, select};
use tracing::{info, warn};
use wasm_bindgen_futures::spawn_local;

use crate::os::Pipe;
use crate::{lang, send_msg, util};

#[derive(Default)]
pub struct WorkerStateExec {
    stop: RefCell<Option<Sender<()>>>,
    stdin: RefCell<Option<Pipe>>,
}

fn state() -> &'static WorkerStateExec {
    &crate::state().exec
}

fn run(
    files: Vec<File>,
    primary_file: String,
    language: String,
    input: Option<Vec<u8>>,
    config: ExecConfig,
) {
    info!("Starting execution of {:?} code", language);

    let (sender, mut receiver) = channel();
    state().stop.borrow_mut().replace(sender);
    let stdin = Pipe::new();
    if let Some(input) = input {
        stdin.write(&input);
        stdin.close();
    }
    state().stdin.borrow_mut().replace(stdin.clone());
    let stdout = Pipe::new();

    spawn_local({
        let stdout = stdout.clone();
        async move {
            let running = lang::run(language, config, files, primary_file, stdin, stdout);
            select! {
                _ = receiver => {
                    info!("Received stop command, cancelling execution");
                    send_msg(WorkerExecResponse::Error("Execution cancelled by user".to_string()));
                }
                res = running.fuse() => {
                    info!("Execution finished");
                    match res {
                        Ok(()) => send_msg(WorkerExecResponse::Success),
                        Err(e) => send_msg(WorkerExecResponse::Error(format!("{e:?}"))),
                    }
                }
            };
        }
    });

    spawn_local(async move {
        loop {
            let len = stdout
                .fill_buf(|buf| {
                    if !buf.is_empty() {
                        util::send_stdout(buf);
                    }
                    buf.len()
                })
                .await;
            if len == 0 {
                break;
            }
        }
    });
}

fn stdin_chunk(chunk: Vec<u8>) {
    if let Some(stdin) = &*state().stdin.borrow_mut() {
        stdin.write(&chunk);
    } else {
        warn!("Received stdin chunk but no pipe is set");
    }
}

fn cancel() {
    if let Some(s) = state().stop.borrow_mut().take() {
        let _ = s.send(());
    } else {
        warn!("Received cancel message but no execution is running");
    }
    if let Some(stdin) = state().stdin.borrow_mut().take() {
        stdin.close();
    }
}

pub fn handle_exec_request(req: WorkerExecRequest) {
    match req {
        WorkerExecRequest::Run {
            files,
            primary_file,
            language,
            input,
            config,
        } => run(files, primary_file, language, input, config),
        WorkerExecRequest::StdinChunk(chunk) => stdin_chunk(chunk),
        WorkerExecRequest::Cancel => cancel(),
    }
}
