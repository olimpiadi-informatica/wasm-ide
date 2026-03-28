use std::cell::RefCell;

use common::{WorkerLSRequest, WorkerLSResponse};
use futures::channel::oneshot::{Sender, channel};
use futures::{FutureExt, select};
use tracing::{debug, info, warn};
use wasm_bindgen_futures::spawn_local;

use crate::os::Pipe;
use crate::{lang, send_msg};

#[derive(Default)]
pub struct WorkerStateLS {
    stop: RefCell<Option<Sender<()>>>,
    stdin: RefCell<Option<Pipe>>,
}

fn state() -> &'static WorkerStateLS {
    &crate::state().ls
}

fn start(lang: String) {
    stop();

    // TODO: wait for previous LS to stop?

    info!("Starting LS for {:?}", lang);

    let (sender, mut receiver) = channel();
    state().stop.borrow_mut().replace(sender);
    let stdin = Pipe::new();
    state().stdin.borrow_mut().replace(stdin.clone());
    let stdout = Pipe::new();
    let stderr = Pipe::new();

    spawn_local({
        let stdout = stdout.clone();
        let stderr = stderr.clone();
        async move {
            let running = lang::run_ls(lang, stdin, stdout, stderr);
            select! {
                _ = receiver => {
                    info!("Received stop command, stopping LS");
                    send_msg(WorkerLSResponse::Stopped);
                }
                res = running.fuse() => {
                    info!("LS finished");
                    match res {
                        Ok(()) => {
                            tracing::warn!("LS exited unexpectedly");
                            send_msg(WorkerLSResponse::Stopped);
                        }
                        Err(e) => send_msg(WorkerLSResponse::Error(format!("{e:?}"))),
                    }
                }
            }
        }
    });

    spawn_local(async move {
        let mut content_length = 0usize;
        let mut line = Vec::new();
        loop {
            stdout.read_until(b'\n', &mut line).await;
            if line.is_empty() {
                break;
            }
            if line.last() != Some(&b'\n') {
                warn!("Partial message from LS");
                continue;
            }
            if line.starts_with(b"Content-Length: ") {
                content_length = std::str::from_utf8(&line[16..line.len() - 2])
                    .ok()
                    .and_then(|s| s.parse::<usize>().ok())
                    .expect("Invalid Content-Length");
            }
            if line == b"\r\n" {
                line.resize(content_length, 0);
                if stdout.read_exact(&mut line).await.is_err() {
                    warn!("Partial message from LS");
                    break;
                }
                let msg = String::from_utf8(line.clone()).unwrap();
                debug!("LS response: {}", msg);
                send_msg(WorkerLSResponse::Message(msg));
            }
        }
    });

    spawn_local(async move {
        let mut line = Vec::new();
        loop {
            stderr.read_until(b'\n', &mut line).await;
            if line.is_empty() {
                break;
            }
            if line.last() != Some(&b'\n') {
                warn!("Partial line from LS stderr");
                continue;
            }
            let msg = String::from_utf8_lossy(&line[..line.len() - 1]);
            debug!("LS stderr: {}", msg);
        }
    });
}

fn message(msg: String) {
    if let Some(stdin) = &*state().stdin.borrow_mut() {
        debug!("Received LS message: {}", msg);
        stdin.write(format!("Content-Length: {}\r\n\r\n", msg.len()).as_bytes());
        stdin.write(msg.as_bytes());
    } else {
        warn!("Received LS message but no pipe is set");
    }
}

fn stop() {
    if let Some(s) = state().stop.borrow_mut().take() {
        let _ = s.send(());
    }
    if let Some(stdin) = state().stdin.borrow_mut().take() {
        stdin.close();
    }
}

pub fn handle_ls_request(req: WorkerLSRequest) {
    match req {
        WorkerLSRequest::Start(lang) => start(lang),
        WorkerLSRequest::Message(msg) => message(msg),
        WorkerLSRequest::Stop => stop(),
    }
}
