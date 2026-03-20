#![allow(deprecated)]
leptos_i18n::load_locales!();

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_channel::{Sender, unbounded};
use common::config::Config;
use common::{
    ExecConfig, File, WorkerExecRequest, WorkerExecResponse, WorkerExecStatus, WorkerLSRequest,
    WorkerLSResponse, WorkerRequest, WorkerResponse, init_logging,
};
use futures_util::FutureExt;
use gloo_net::http::Request;
use leptos::prelude::*;
use tracing::{debug, info, warn};
use wasm_bindgen_futures::spawn_local;

mod backend;
mod editor;
mod editor_dir;
mod editor_view;
mod enum_select;
mod output;
mod settings;
mod status_view;
mod util;
mod workspace;

use crate::backend::{CombinedBackend, DynBackend, RemoteBackend, WorkerBackend};
use crate::editor_dir::EditorDirController;
use crate::editor_view::EditorView;
use crate::enum_select::EnumSelect;
use crate::i18n::*;
use crate::output::OutputView;
use crate::settings::{InputMode, Settings, SettingsProvider, set_input_mode, use_settings};
use crate::status_view::StatusView;
use crate::util::{Icon, get_input_mode};
use crate::workspace::WorkspaceSelector;

#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub stdout: Vec<u8>,
    pub compile_stderr: Vec<u8>,
    pub stderr: Vec<u8>,
}

type FetchingCompilerProgress = HashMap<String, u64>;

#[derive(Clone, Debug)]
struct RunState {
    exec: StateExec,
    ls: StateLS,
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
        !self.is_running()
    }

    fn can_stop(&self) -> bool {
        match self.exec {
            StateExec::Ready
            | StateExec::Complete { .. }
            | StateExec::Processing { stopping: true, .. } => false,
            StateExec::Processing {
                stopping: false, ..
            } => true,
        }
    }

    fn is_running(&self) -> bool {
        match self.exec {
            StateExec::Ready | StateExec::Complete { .. } => false,
            StateExec::Processing { .. } => true,
        }
    }
}

fn handle_message(
    msg: WorkerResponse,
    state: RwSignal<RunState>,
    fetching_compiler_progress: RwSignal<FetchingCompilerProgress>,
    ls_message_chan: &Sender<WorkerLSResponse>,
) -> Result<()> {
    debug!("{msg:?}");
    match msg {
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

    match (msg, &mut state.exec) {
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
            state.exec = StateExec::Complete {
                outcome: std::mem::take(outcome),
                error: None,
            };
        }

        (WorkerExecResponse::Error(s), StateExec::Processing { outcome, .. }) => {
            state.exec = StateExec::Complete {
                outcome: std::mem::take(outcome),
                error: Some(s),
            };
        }

        (msg, _) => {
            warn!(
                "unexpected msg & state combination: {msg:?} {:?}",
                state.exec
            );
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

    let msg2 = msg.clone();
    match (msg, &mut state.ls) {
        (WorkerLSResponse::FetchingCompiler, StateLS::Requested | StateLS::Error(_)) => {
            state.ls = StateLS::FetchingCompiler;
        }

        (WorkerLSResponse::Started, StateLS::Requested | StateLS::FetchingCompiler) => {
            state.ls = StateLS::Running;
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
            state.ls = StateLS::Error(msg);
        }

        (msg, _) => {
            warn!("unexpected msg & state combination: {msg:?} {:?}", state.ls);
        }
    }
    Ok(())
}

#[component]
fn StoragePersistView() -> impl IntoView {
    let i18n = use_i18n();

    let (task, handle) = common::opfs::persist().remote_handle();
    spawn_local(task);

    view! {
        <Await future=handle let:(&persist)>
            <Show when=move || !persist>
                <div
                    class:message
                    class:is-warning
                    style:position="absolute"
                    style:bottom="1px"
                    style:right="1px"
                    style:z-index="100"
                >
                    <div class:message-body>{t!(i18n, storage_denied)}</div>
                </div>
            </Show>
        </Await>
    }
}

#[component]
fn App() -> impl IntoView {
    let i18n = use_i18n();

    let state = RwSignal::new(RunState {
        exec: StateExec::Ready,
        ls: StateLS::Ready,
    });

    let (ls_sender, ls_receiver) = unbounded();

    let fetching_compiler_progress = RwSignal::new(FetchingCompilerProgress::default());

    let SettingsProvider {
        input_mode,
        mem_limit,
        time_limit,
        ..
    } = use_settings();

    let backend = expect_context::<DynBackend>();
    backend.set_callback(Arc::new({
        let ls_sender = ls_sender.clone();
        move |msg| {
            handle_message(msg, state, fetching_compiler_progress, &ls_sender).unwrap();
        }
    }));

    let send_worker_message = move |msg: WorkerRequest| {
        backend.clone().send_message(msg);
    };

    let workspace = RwSignal::new(None);

    let code = EditorDirController::new(Signal::derive(move || {
        workspace
            .read()
            .as_ref()
            .map(|ws| format!("workspace/{ws}/code"))
    }));

    let stdin = EditorDirController::new(Signal::derive(move || {
        workspace
            .read()
            .as_ref()
            .map(|ws| format!("workspace/{ws}/stdin"))
    }));

    let backend = expect_context::<DynBackend>();
    let language = Memo::new(move |old| {
        code.open_filename()
            .get()
            .and_then(|f| {
                let ext = f.split('.').next_back().unwrap_or("");
                backend
                    .languages()
                    .iter()
                    .find(|lang| lang.extensions.iter().any(|e| e == ext))
                    .map(|lang| lang.name.clone())
            })
            .or(old.cloned())
            .unwrap_or("C++".to_owned())
    });

    let disable_start = Memo::new(move |_| state.with(|s| !s.can_start()));
    let disable_stop = Memo::new(move |_| state.with(|s| !s.can_stop()));
    let is_running = Memo::new(move |_| state.with(|s| s.is_running()));

    {
        let send_worker_message = send_worker_message.clone();
        let ls_sender = ls_sender.clone();
        Effect::new(move |_| {
            let lang = language.get();
            info!("Requesting language server for {lang:?}");
            state.update(|s| {
                s.ls = StateLS::Requested;
            });
            ls_sender.try_send(WorkerLSResponse::Stopped).unwrap();
            send_worker_message(WorkerLSRequest::Start(lang).into());
        });
    }

    let do_run = {
        let send_worker_message = send_worker_message.clone();
        Callback::new(move |()| {
            let Some(ws) = workspace.get_untracked() else {
                return;
            };
            let Some(primary_file) = code.open_filename().get_untracked() else {
                return;
            };
            let primary_file = primary_file
                .split('/')
                .next_back()
                .expect("invalid primary file")
                .to_string();

            match &mut state.write().exec {
                exec @ (StateExec::Ready | StateExec::Complete { .. }) => {
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
            let input_mode = get_input_mode(input_mode, language.into());
            spawn_local(async move {
                code.wait_sync().await;
                if input_mode == InputMode::FullInteractive {
                    stdin.set_text("");
                }
                let input = stdin.get_text();
                let (input, addn_msg) = match input_mode {
                    InputMode::MixedInteractive => (
                        None,
                        Some(WorkerExecRequest::StdinChunk(input.into_bytes())),
                    ),
                    InputMode::FullInteractive => (None, None),
                    InputMode::Batch => (Some(input.into_bytes()), None),
                };

                let mut files = Vec::new();
                let dir = common::opfs::open_dir(&format!("workspace/{ws}/code"), false).await;
                for name in dir.list_entries().await {
                    let file = dir.open_file(&name, false).await;
                    let content = String::from_utf8(file.read().await).unwrap();
                    files.push(File { name, content });
                }

                info!("Requesting execution");
                let lng = language.get_untracked();
                send_worker_message(
                    WorkerExecRequest::CompileAndRun {
                        files,
                        primary_file,
                        language: lng,
                        input,
                        config: ExecConfig {
                            mem_limit: mem_limit.get_untracked().map(|x| x * 16),
                            time_limit: time_limit.get_untracked(),
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
            match &mut state.write().exec {
                StateExec::Processing { stopping, .. } => {
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
        let backend = expect_context::<DynBackend>();
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
                <WorkspaceSelector active=workspace readonly=is_running />
                <div class="is-flex-grow-1" />
                <Show when=move || backend.has_dynamic_io(&language.get())>
                    <EnumSelect value=(input_mode, SignalSetter::map(set_input_mode)) />
                </Show>
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

    let disable_input_editor = Memo::new(move |_| {
        is_running.get()
            || get_input_mode(input_mode, language.into()) == InputMode::FullInteractive
    });

    view! {
        <StatusView state fetching_compiler_progress />
        <StoragePersistView />
        <div class:is-flex class:is-flex-direction-column style:height="100dvh">
            {navbar}
            <EditorView
                ls_receiver=ls_receiver
                send_worker_message=Callback::new(send_worker_message)
                code=code
                stdin=stdin
                ctrl_enter=do_run
                language=language
                code_readonly=is_running
                input_readonly=disable_input_editor
                disable_additional_input=disable_stop
            />
            <OutputView state />
        </div>
    }
}

#[component]
fn ConfigAndBackendProvider(children: Children) -> impl IntoView {
    let config = async {
        let res = Request::get("config.json").send().await.unwrap();
        assert!(res.ok(), "could not load config: {}", res.status());
        let config: Config = res.json().await.unwrap();

        let worker = WorkerBackend::new().await;
        let backend: DynBackend = if let Some(remote_eval) = &config.remote_eval {
            let remote = RemoteBackend::new(remote_eval.clone()).await.unwrap();
            CombinedBackend::new(worker, remote)
        } else {
            worker
        };

        (config, backend)
    };

    let (task, handle) = config.remote_handle();
    spawn_local(task);

    view! {
        <Suspense fallback=|| {
            view! { <p>"Loading config..."</p> }
        }>
            {Suspend::new(async move {
                let (config, backend) = handle.await;
                provide_context::<Config>(config);
                provide_context::<DynBackend>(backend);
                children()
            })}

        </Suspense>
    }
}

fn main() {
    init_logging();

    mount_to_body(move || {
        SettingsProvider::install();
        view! {
            <I18nContextProvider>
                <ConfigAndBackendProvider>
                    <App />
                </ConfigAndBackendProvider>
            </I18nContextProvider>
        }
    })
}
