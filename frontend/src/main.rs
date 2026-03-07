#![allow(deprecated)]
leptos_i18n::load_locales!();

use std::collections::HashMap;
use std::ops::DerefMut;

use anyhow::Result;
use async_channel::{unbounded, Sender};
use common::{
    init_logging, ExecConfig, WorkerExecRequest, WorkerExecResponse, WorkerExecStatus,
    WorkerLSRequest, WorkerLSResponse, WorkerRequest, WorkerResponse,
};
use editor_view::EditorView;
use futures_util::FutureExt;
use i18n::*;
use leptos::prelude::*;
use send_wrapper::SendWrapper;
use settings::{set_input_mode, set_language, use_settings, SettingsProvider};
use tracing::{debug, info, warn};
use util::Icon;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{MessageEvent, SubmitEvent, Worker, WorkerOptions, WorkerType};

mod editor;
mod editor_dir;
mod editor_view;
mod enum_select;
mod output;
mod settings;
mod status_view;
mod util;

use crate::editor_dir::EditorDirController;
use crate::enum_select::EnumSelect;
use crate::output::OutputView;
use crate::settings::{InputMode, Settings};
use crate::status_view::StatusView;

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
fn WorkspaceSelector(
    active: RwSignal<Option<String>>,
    #[prop(into)] readonly: Signal<bool>,
) -> impl IntoView {
    // TODO(veluca): Allow overriding the default code, possibly at runtime.
    let starting_code = include_str!("../default_code.txt");
    let starting_stdin = include_str!("../default_stdin.txt");

    let i18n = use_i18n();
    let workspaces = RwSignal::new(Vec::new());
    let open = RwSignal::new(false);
    let new_ws = RwSignal::new(String::new());

    spawn_local(async move {
        let dir = common::opfs::open_dir("workspace", true).await;
        let entries = dir.list_entries().await;
        workspaces.set(entries);
    });

    let new_workspace = move |ev: SubmitEvent| {
        ev.prevent_default();
        let name = new_ws.get_untracked();
        if name.is_empty() {
            return;
        }
        spawn_local(async move {
            let code =
                common::opfs::open_file(&format!("workspace/{name}/code/main.cpp"), true).await;
            code.write(starting_code.as_bytes()).await;
            let stdin =
                common::opfs::open_file(&format!("workspace/{name}/stdin/input.txt"), true).await;
            stdin.write(starting_stdin.as_bytes()).await;

            workspaces.update(|w| w.push(name.clone()));
            active.set(Some(name));
            open.set(false);
            new_ws.set(String::new());
        });
    };

    let render_ws = move |ws: String| {
        let ws2 = ws.clone();
        let ws3 = ws.clone();
        view! {
            <a on:click=move |_| {
                active.set(Some(ws2.clone()));
                open.set(false);
            }>{ws}</a>
            <Icon
                icon=icondata::BiTrashSolid
                class:is-clickable
                style:height="1em"
                style:width="1em"
                on:click=move |_| {
                    workspaces.update(|w| w.retain(|x| x != &ws3));
                    active
                        .update(|a| {
                            if a.as_ref() == Some(&ws3) {
                                *a = None;
                            }
                        });
                    let ws3 = ws3.clone();
                    spawn_local(async move {
                        let dir = common::opfs::open_dir("workspace", true).await;
                        dir.remove_entry(&ws3, true).await;
                    });
                }
            />
        }
    };

    view! {
        <button class:button on:click=move |_| open.set(true) disabled=readonly>
            {move || active.get().unwrap_or_else(|| t_string!(i18n, choose_workspace).into())}
        </button>

        <div class:modal class:is-active=open>
            <div class="modal-background" on:click=move |_| open.set(false) />
            <div class="modal-card">
                <header class="modal-card-head">
                    <p class="modal-card-title">{t!(i18n, workspaces)}</p>
                    <button class="delete" aria-label="close" on:click=move |_| open.set(false) />
                </header>
                <section
                    class="modal-card-body"
                    style:display="grid"
                    style:grid-template-columns="auto 1em"
                >
                    <form
                        style:grid-column="span 2"
                        class:is-flex
                        class:is-column-gap-2
                        class:is-align-items-center
                        class:mb-6
                        on:submit=new_workspace
                    >
                        <input
                            class="input"
                            type="text"
                            placeholder=move || t_string!(i18n, workspace_name)
                            bind:value=new_ws
                        />
                        <button class="button is-primary" type="submit">
                            {t!(i18n, create_workspace)}
                        </button>
                    </form>
                    <For each=move || workspaces.get() key=|w| w.clone() children=render_ws />
                </section>
            </div>
        </div>
    }
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

            let Some(ws) = workspace.get_untracked() else {
                return;
            };

            let send_worker_message = send_worker_message.clone();
            spawn_local(async move {
                code.wait_sync().await;
                if input_mode.get_untracked() == InputMode::FullInteractive {
                    stdin.set_text("");
                }
                let input = stdin.get_text();
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
                        workspace: ws,
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
                <WorkspaceSelector active=workspace readonly=is_running />
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
        <StoragePersistView />
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
        view! {
            <I18nContextProvider>
                <App />
            </I18nContextProvider>
        }
    })
}
