#![allow(deprecated)]
leptos_i18n::load_locales!();

use std::collections::{HashMap, HashSet};
use std::ops::DerefMut;

use anyhow::Result;
use async_channel::{unbounded, Sender};
use common::{
    init_logging, ExecConfig, File, WorkerExecRequest, WorkerExecResponse, WorkerExecStatus,
    WorkerLSRequest, WorkerLSResponse, WorkerRequest, WorkerResponse,
};
use editor_view::EditorView;
use i18n::*;
use leptos::prelude::*;
use send_wrapper::SendWrapper;
use settings::{set_input_mode, set_language, use_settings, SettingsProvider};
use tracing::{debug, info, warn};
use util::Icon;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{MessageEvent, Worker, WorkerOptions, WorkerType};

use crate::settings::InputMode;

mod editor;
mod editor_view;
mod enum_select;
mod output;
mod settings;
mod status_view;
mod util;

use crate::editor::EditorText;
use crate::enum_select::EnumSelect;
use crate::output::OutputView;
use crate::settings::Settings;
use crate::status_view::StatusView;
use crate::util::load;

#[derive(Default)]
struct LargeFileSet(HashSet<String>);

#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub stdout: Vec<u8>,
    pub compile_stderr: Vec<u8>,
    pub stderr: Vec<u8>,
}

// TODO(Virv12): Can we always have the progress info?
type FetchingCompilerProgress = HashMap<String, Option<(u64, u64)>>;

#[derive(Clone, Debug)]
enum RunState {
    Loading,
    Ready { exec: StateExec, ls: StateLS },
}

#[derive(Clone, Debug)]
enum StateExec {
    Ready,
    Processing {
        status: Option<WorkerExecStatus>,
        outcome: Outcome,
        stopping: bool,
    },
    Complete {
        outcome: Outcome,
        error: Option<String>,
    },
}

#[derive(Clone, Debug)]
enum StateLS {
    Ready,
    Requested,
    FetchingCompiler,
    Running,
    Error(String),
}

impl RunState {
    fn can_start(&self) -> bool {
        let exec = match self {
            RunState::Ready { exec, .. } => exec,
            _ => return false,
        };
        match exec {
            StateExec::Processing { .. } => false,
            StateExec::Ready | StateExec::Complete { .. } => true,
        }
    }

    fn can_stop(&self) -> bool {
        let exec = match self {
            RunState::Ready { exec, .. } => exec,
            _ => return false,
        };
        match exec {
            StateExec::Ready
            | StateExec::Complete { .. }
            | StateExec::Processing { stopping: true, .. } => false,
            StateExec::Processing {
                stopping: false, ..
            } => true,
        }
    }

    fn is_running(&self) -> bool {
        let exec = match self {
            RunState::Ready { exec, .. } => exec,
            _ => return false,
        };
        match exec {
            StateExec::Ready | StateExec::Complete { .. } => false,
            StateExec::Processing { .. } => true,
        }
    }
}

#[component]
fn StorageErrorView() -> impl IntoView {
    let i18n = use_i18n();
    let large_files = expect_context::<RwSignal<LargeFileSet>>();
    view! {
        <div
            class:message
            class:is-warning
            class:is-hidden=move || large_files.read().0.is_empty()
            style:position="absolute"
            style:bottom="1px"
            style:right="1px"
            style:z-index="100"
        >
            <div class:message-body>{t!(i18n, files_too_big)}</div>
        </div>
    }
}

fn handle_message(
    msg: JsValue,
    state: RwSignal<RunState>,
    fetching_compiler_progress: RwSignal<FetchingCompilerProgress>,
    ls_message_chan: &Sender<WorkerLSResponse>,
) -> Result<()> {
    let msg = msg.dyn_into::<MessageEvent>().unwrap().data();
    let msg = match serde_wasm_bindgen::from_value::<WorkerResponse>(msg) {
        Ok(msg) => msg,
        Err(e) => {
            warn!("invalid message from worker: {e}");
            return Ok(());
        }
    };
    debug!("{msg:?}");
    match msg {
        WorkerResponse::Ready => {
            state.set(RunState::Ready {
                exec: StateExec::Ready,
                ls: StateLS::Ready,
            });
        }
        WorkerResponse::Execution(msg) => {
            handle_exec_message(msg, state)?;
        }
        WorkerResponse::LS(msg) => {
            handle_ls_message(msg, state, ls_message_chan)?;
        }
        WorkerResponse::FetchingCompiler(name, progress) => {
            fetching_compiler_progress.update(|x| {
                x.insert(name, progress);
            });
        }
        WorkerResponse::CompilerFetchDone(name) => {
            fetching_compiler_progress.update(|x| {
                x.remove(&name);
            });
        }
    };
    Ok(())
}

fn handle_exec_message(msg: WorkerExecResponse, state: RwSignal<RunState>) -> Result<()> {
    let mut state = state.write();
    let exec = match state.deref_mut() {
        RunState::Ready { exec, .. } => exec,
        _ => {
            warn!("received execution message while not ready: {msg:?}");
            return Ok(());
        }
    };

    match (msg, &mut *exec) {
        (WorkerExecResponse::Status(new), StateExec::Processing { status, .. }) => {
            *status = Some(new);
        }

        (
            WorkerExecResponse::CompilationMessageChunk(chunk),
            StateExec::Processing { outcome, .. },
        ) => {
            outcome.compile_stderr.extend_from_slice(&chunk);
        }
        (WorkerExecResponse::StdoutChunk(chunk), StateExec::Processing { outcome, .. }) => {
            outcome.stdout.extend_from_slice(&chunk);
        }
        (WorkerExecResponse::StderrChunk(chunk), StateExec::Processing { outcome, .. }) => {
            outcome.stderr.extend_from_slice(&chunk);
        }

        (WorkerExecResponse::Success, StateExec::Processing { outcome, .. }) => {
            *exec = StateExec::Complete {
                outcome: std::mem::take(outcome),
                error: None,
            };
        }

        (WorkerExecResponse::Error(s), StateExec::Processing { outcome, .. }) => {
            *exec = StateExec::Complete {
                outcome: std::mem::take(outcome),
                error: Some(s),
            };
        }

        (msg, _) => {
            warn!("unexpected msg & state combination: {msg:?} {exec:?}");
        }
    };

    Ok(())
}

fn handle_ls_message(
    msg: WorkerLSResponse,
    state: RwSignal<RunState>,
    ls_message_chan: &Sender<WorkerLSResponse>,
) -> Result<()> {
    let mut state = state.write();
    let ls = match state.deref_mut() {
        RunState::Ready { ls, .. } => ls,
        _ => {
            warn!("received execution message while not ready: {msg:?}");
            return Ok(());
        }
    };

    let msg2 = msg.clone();
    match (msg, &mut *ls) {
        (WorkerLSResponse::FetchingCompiler, StateLS::Requested | StateLS::Error(_)) => {
            *ls = StateLS::FetchingCompiler;
        }

        (WorkerLSResponse::Started, StateLS::Requested | StateLS::FetchingCompiler) => {
            *ls = StateLS::Running;
            ls_message_chan.try_send(msg2)?;
        }

        (WorkerLSResponse::Message(_), StateLS::Running) => {
            ls_message_chan.try_send(msg2)?;
        }

        (WorkerLSResponse::Stopped, StateLS::Requested) => {}

        (
            WorkerLSResponse::Error(msg),
            StateLS::Requested | StateLS::FetchingCompiler | StateLS::Running,
        ) => {
            *ls = StateLS::Error(msg);
        }

        (msg, _) => {
            warn!("unexpected msg & state combination: {msg:?} {ls:?}");
        }
    }
    Ok(())
}

#[component]
fn App() -> impl IntoView {
    let options = WorkerOptions::default();
    options.set_type(WorkerType::Module);
    let worker =
        Worker::new_with_options("./worker_loader.js", &options).expect("could not start worker");

    let i18n = use_i18n();

    let state = RwSignal::new(RunState::Loading);

    let (ls_sender, ls_receiver) = unbounded();

    let fetching_compiler_progress = RwSignal::new(FetchingCompilerProgress::default());

    let SettingsProvider {
        language,
        input_mode,
        mem_limit,
        ..
    } = use_settings();

    worker.set_onmessage(Some(
        Closure::<dyn Fn(_)>::new({
            let ls_sender = ls_sender.clone();
            move |msg| {
                handle_message(msg, state, fetching_compiler_progress, &ls_sender).unwrap();
            }
        })
        .into_js_value()
        .unchecked_ref(),
    ));

    let send_worker_message = {
        let worker = SendWrapper::new(worker);
        move |msg: WorkerRequest| {
            debug_assert!(
                matches!(*state.read_untracked(), RunState::Ready { .. }),
                "sending message to worker while not ready: {msg:?}"
            );
            debug!("send to worker: {:?}", msg);
            let js_msg = serde_wasm_bindgen::to_value(&msg).expect("invalid message to worker");
            worker.post_message(&js_msg).expect("worker died");
        }
    };

    // TODO(veluca): Allow overriding the default code, possibly at runtime.
    let starting_code = include_str!("../default_code.txt");
    let code =
        RwSignal::new_local(load("code").unwrap_or_else(|| EditorText::from_str(starting_code)));

    let starting_stdin = include_str!("../default_stdin.txt");

    let stdin =
        RwSignal::new_local(load("stdin").unwrap_or_else(|| EditorText::from_str(starting_stdin)));

    let disable_start = Memo::new(move |_| state.with(|s| !s.can_start()));
    let disable_stop = Memo::new(move |_| state.with(|s| !s.can_stop()));
    let is_running = Memo::new(move |_| state.with(|s| s.is_running()));

    {
        let worker_ready = Memo::new(move |_| matches!(*state.read(), RunState::Ready { .. }));

        let send_worker_message = send_worker_message.clone();
        let ls_sender = ls_sender.clone();
        Effect::new(move |_| {
            if !worker_ready.get() {
                return;
            }

            let lang = language.get();
            info!("Requesting language server for {lang:?}");
            state.update(|s| {
                if let RunState::Ready { ls, .. } = s {
                    *ls = StateLS::Requested;
                }
            });
            ls_sender.try_send(WorkerLSResponse::Stopped).unwrap();
            send_worker_message(WorkerLSRequest::Start(lang).into());
        });
    }

    let do_run = {
        let send_worker_message = send_worker_message.clone();
        Callback::new(move |()| {
            match state.write().deref_mut() {
                RunState::Ready {
                    exec: exec @ (StateExec::Ready | StateExec::Complete { .. }),
                    ..
                } => {
                    *exec = StateExec::Processing {
                        status: None,
                        outcome: Outcome::default(),
                        stopping: false,
                    };
                }
                _ => {
                    warn!("asked to run while already running");
                    return;
                }
            }

            let send_worker_message = send_worker_message.clone();
            spawn_local(async move {
                if input_mode.get_untracked() == InputMode::FullInteractive {
                    stdin.set(EditorText::from_str(""));
                }
                code.with_untracked(|x| x.await_all_changes()).await;
                stdin.with_untracked(|x| x.await_all_changes()).await;
                let code = code.with_untracked(|x| x.text().clone());
                let input = stdin.with_untracked(|x| x.text().clone());
                let (input, addn_msg) = match input_mode.get_untracked() {
                    InputMode::MixedInteractive => (
                        None,
                        Some(WorkerExecRequest::StdinChunk(input.into_bytes())),
                    ),
                    InputMode::FullInteractive => (None, None),
                    InputMode::Batch => (Some(input.into_bytes()), None),
                };

                info!("Requesting execution");
                let lng = language.get_untracked();
                send_worker_message(
                    WorkerExecRequest::CompileAndRun {
                        files: vec![File {
                            name: format!("solution.{}", lng.ext()),
                            content: code,
                        }],
                        language: lng,
                        input,
                        config: ExecConfig {
                            mem_limit: mem_limit.get_untracked().map(|x| x * 16),
                        },
                    }
                    .into(),
                );
                if let Some(addn_msg) = addn_msg {
                    send_worker_message(addn_msg.into());
                }
            });
        })
    };

    let do_stop = {
        let send_worker_message = send_worker_message.clone();
        move |_| {
            match state.write().deref_mut() {
                RunState::Ready {
                    exec: StateExec::Processing { stopping, .. },
                    ..
                } => {
                    *stopping = true;
                }
                _ => {
                    warn!("asked to stop while not running");
                    return;
                }
            }
            info!("Stopping execution");
            send_worker_message(WorkerExecRequest::Cancel.into());
        }
    };

    let navbar = {
        let do_stop = do_stop.clone();
        view! {
            <div
                class:is-flex
                class:is-flex-direction-row
                class:is-align-items-center
                class:is-column-gap-2
                class:my-2
                class:mx-3
            >
                <Settings />
                <EnumSelect value=(language, SignalSetter::map(set_language)) />
                <div class="is-flex-grow-1" />
                <EnumSelect value=(input_mode, SignalSetter::map(set_input_mode)) />
                <Show when=move || is_running.get()>
                    <button
                        class:has-icons-left
                        class:button
                        class:is-danger
                        class:mr-1
                        style:width="8em"
                        disabled=disable_stop
                        on:click=do_stop.clone()
                    >
                        <Icon class:icon class:is-left class:mr-1 icon=icondata::AiCloseOutlined />
                        {t!(i18n, stop)}
                    </button>
                </Show>
                <Show when=move || !is_running.get()>
                    <button
                        class:has-icons-left
                        class:button
                        class:is-success
                        class:mr-1
                        style:width="8em"
                        disabled=disable_start
                        on:click=move |_| {
                            if !disable_start.get() {
                                do_run.run(())
                            }
                        }
                    >
                        <Icon
                            class:icon
                            class:is-left
                            class:mr-1
                            icon=icondata::AiCaretRightFilled
                        />
                        {t!(i18n, run)}
                    </button>
                </Show>
            </div>
        }
    };

    let disable_input_editor =
        Memo::new(move |_| is_running.get() || input_mode.get() == InputMode::FullInteractive);

    view! {
        <StatusView state fetching_compiler_progress />
        <StorageErrorView />
        <div class:is-flex class:is-flex-direction-column style:height="100dvh">
            {navbar}
            <EditorView
                ls_receiver=ls_receiver
                send_worker_message=Callback::new(send_worker_message)
                code=code
                stdin=stdin
                ctrl_enter=do_run
                code_readonly=is_running
                input_readonly=disable_input_editor
                disable_additional_input=disable_stop
            />
            <OutputView state />
        </div>
    }
}

fn main() {
    init_logging();

    mount_to_body(move || {
        SettingsProvider::install();
        let files = RwSignal::new(LargeFileSet::default());
        provide_context(files);
        view! {
            <I18nContextProvider>
                <App />
            </I18nContextProvider>
        }
    })
}
