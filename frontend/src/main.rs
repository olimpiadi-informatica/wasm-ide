#![allow(deprecated)]
leptos_i18n::load_locales!();

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;

use anyhow::Result;
use common::config::Config;
use common::{
    ExecConfig, File, WorkerExecRequest, WorkerExecResponse, WorkerExecStatus, WorkerLSRequest,
    WorkerLSResponse, WorkerRequest, WorkerResponse, init_logging,
};
use futures_channel::mpsc::{UnboundedSender, unbounded};
use gloo_net::http::Request;
use leptos::prelude::*;
use leptos::task::{spawn_local, spawn_local_scoped};
use tracing::{info, warn};

mod backend;
mod contest_api;
mod editor;
mod editor_dir;
mod editor_view;
mod enum_select;
mod output;
mod settings;
mod status_view;
mod util;
mod workspace;

use crate::backend::{JsBackend, RemoteBackend, WorkerBackend};
use crate::contest_api::SubmitStatus;
use crate::editor_dir::EditorDirController;
use crate::editor_view::EditorView;
use crate::enum_select::EnumSelect;
use crate::i18n::*;
use crate::output::OutputView;
use crate::settings::{InputMode, Settings, SettingsProvider, set_input_mode, use_settings};
use crate::status_view::StatusView;
use crate::util::{Icon, get_input_mode};
use crate::workspace::{WorkspaceConfig, WorkspaceSelector};

#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub stdout: Vec<u8>,
    pub compile_stderr: Vec<u8>,
    pub stderr: Vec<u8>,
}

type FetchingCompilerProgress = HashMap<String, u64>;

#[derive(Clone)]
struct RunState {
    exec: StateExec,
    ls: StateLS,
    submit: StateSubmit,
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

#[derive(Clone)]
enum StateSubmit {
    Ready,
    Submitting(Option<backend::Callback>),
    Complete(SubmitStatus),
    Error(String),
}

impl RunState {
    fn can_start(&self) -> bool {
        !self.is_running() && !matches!(self.submit, StateSubmit::Submitting(_))
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
    ls_message_chan: &UnboundedSender<WorkerLSResponse>,
) -> Result<()> {
    match msg {
        WorkerResponse::Execution(msg) => handle_exec_message(msg, state)?,
        WorkerResponse::LS(msg) => handle_ls_message(msg, state, ls_message_chan)?,
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

    if let StateSubmit::Submitting(Some(proxy)) = &state.submit {
        proxy(WorkerResponse::Execution(msg));
        return Ok(());
    }

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
    ls_message_chan: &UnboundedSender<WorkerLSResponse>,
) -> Result<()> {
    let mut state = state.write();

    let msg2 = msg.clone();
    match (msg, &mut state.ls) {
        (WorkerLSResponse::FetchingCompiler, StateLS::Requested | StateLS::Error(_)) => {
            state.ls = StateLS::FetchingCompiler;
        }

        (WorkerLSResponse::Started, StateLS::Requested | StateLS::FetchingCompiler) => {
            state.ls = StateLS::Running;
            ls_message_chan.unbounded_send(msg2)?;
        }

        (WorkerLSResponse::Message(_), StateLS::Running) => {
            ls_message_chan.unbounded_send(msg2)?;
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
    let settings = use_settings();
    let ok = LocalResource::new(move || async move {
        if settings.persist_storage.get() {
            common::opfs::persist().await
        } else {
            true
        }
    });

    view! {
        <Show when=move || ok.get() == Some(false)>
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
    }
}

#[component]
fn App() -> impl IntoView {
    let i18n = use_i18n();

    let state = RwSignal::new(RunState {
        exec: StateExec::Ready,
        ls: StateLS::Ready,
        submit: StateSubmit::Ready,
    });

    // TODO(virv): a bit of bad design, used to make the StateSubmit::Submitting proxy work
    provide_context::<RwSignal<RunState>>(state);

    let (ls_sender, ls_receiver) = unbounded();

    let fetching_compiler_progress = RwSignal::new(FetchingCompilerProgress::default());

    let SettingsProvider {
        input_mode,
        mem_limit,
        time_limit,
        ..
    } = use_settings();

    backend::set_callback(Arc::new({
        let ls_sender = ls_sender.clone();
        move |msg| handle_message(msg, state, fetching_compiler_progress, &ls_sender).unwrap()
    }));

    let workspace = RwSignal::new(None);
    let workspace_config = LocalResource::new(move || {
        let workspace = workspace.get();
        async move {
            let ws = workspace?;
            let config_file =
                common::opfs::open_file(&format!("workspace/{ws}/config.json"), false).await;
            let config = config_file.read().await;
            serde_json::from_slice::<WorkspaceConfig>(&config).ok()
        }
    });
    let task_score = LocalResource::new(move || {
        let workspace_config = workspace_config.get();
        async move {
            let config = workspace_config.flatten()?;
            let task = config.task?;
            let api = contest_api::get()?;
            api.task_score(&task).await.ok()
        }
    });

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

    let language = Memo::new(move |old| {
        code.open_filename()
            .get()
            .and_then(|f| {
                let ext = f.split('.').next_back().unwrap_or("");
                backend::languages()
                    .iter()
                    .find(|lang| lang.extensions.iter().any(|e| e == ext))
                    .map(|lang| lang.name.clone())
            })
            .or(old.cloned())
            .unwrap_or("C++".to_owned())
    });

    // TODO(virv): remove this
    let send_worker_message = move |msg: WorkerRequest| {
        backend::for_lang(&language.get_untracked()).send_message(msg);
    };

    let disable_start = Memo::new(move |_| state.with(|s| !s.can_start()));
    let disable_stop = Memo::new(move |_| state.with(|s| !s.can_stop()));
    let is_running = Memo::new(move |_| state.with(|s| s.is_running()));

    {
        let ls_sender = ls_sender.clone();
        Effect::new(move |old: Option<String>| {
            let lang = language.get();
            info!("Requesting language server for {lang:?}");
            state.update(|s| {
                s.ls = StateLS::Requested;
            });
            if let Some(old) = old {
                backend::for_lang(&old).send_message(WorkerLSRequest::Stop.into());
            }
            ls_sender.unbounded_send(WorkerLSResponse::Stopped).unwrap();
            send_worker_message(WorkerLSRequest::Start(lang.clone()).into());
            lang
        });
    }

    let do_run = Callback::new(move |()| {
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

        let input_mode = get_input_mode(
            input_mode.get_untracked(),
            language.read_untracked().deref(),
        );
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
                let content = file.read().await;
                files.push(File { name, content });
            }

            info!("Requesting execution");
            let lng = language.get_untracked();
            send_worker_message(
                WorkerExecRequest::Run {
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
    });

    let do_stop = move |_| {
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
    };

    let disable_submit = Memo::new(move |_| {
        state.with(|s| matches!(s.submit, StateSubmit::Submitting(_)) || s.is_running())
            || workspace.get().is_none()
            || workspace_config
                .get()
                .map(|cfg| cfg.and_then(|cfg| cfg.task).is_none())
                .unwrap_or(false)
    });

    let do_submit = Callback::new(move |_| {
        let Some(ws) = workspace.get_untracked() else {
            warn!("asked to submit without a workspace");
            return;
        };
        let Some(primary_file) = code.open_filename().get_untracked() else {
            warn!("asked to submit without a primary file");
            return;
        };
        let Some(api) = contest_api::get() else {
            warn!("asked to submit without contest api");
            return;
        };
        let primary_file = primary_file
            .split('/')
            .next_back()
            .unwrap_or(&primary_file)
            .to_string();
        state.update(|s| {
            s.submit = StateSubmit::Submitting(None);
        });

        spawn_local_scoped(async move {
            code.wait_sync().await;

            let config_file =
                common::opfs::open_file(&format!("workspace/{ws}/config.json"), false).await;
            let config = config_file.read().await;
            let config: WorkspaceConfig = serde_json::from_slice(&config).unwrap();

            let mut files = Vec::new();
            let dir = common::opfs::open_dir(&format!("workspace/{ws}/code"), false).await;
            for name in dir.list_entries().await {
                let file = dir.open_file(&name, false).await;
                let content = file.read().await;
                files.push((name, content));
            }

            let res = api
                .submit(
                    config
                        .task
                        .as_deref()
                        .expect("submit without connected task"),
                    &language.get_untracked(),
                    &primary_file,
                    files,
                )
                .await;
            state.update(|s| {
                s.submit = match res.as_ref() {
                    Ok(status) => StateSubmit::Complete(status.clone()),
                    Err(err) => StateSubmit::Error(err.to_string()),
                };
            });
            if res.is_ok() {
                task_score.refetch();
            }
            if let Err(err) = res {
                warn!("submit failed: {err:?}");
            }
        });
    });

    let navbar = view! {
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
            <ShowLet some=move || task_score.get().flatten() let:((score, max_score))>
                <div
                    class:is-flex
                    class:is-align-items-center
                    class:px-3
                    class:py-2
                    class:is-size-7
                    style:background-color="var(--bulma-scheme-main-bis)"
                    style:border="1px solid var(--bulma-border)"
                    style:border-radius="0.9rem"
                    style:gap="0.75rem"
                    style:min-width="11rem"
                >
                    <div>
                        <div
                            class:is-uppercase
                            class:has-text-weight-semibold
                            style:font-size="0.65rem"
                            style:letter-spacing="0.08em"
                            style:line-height="1"
                        >
                            {t!(i18n, task_score)}
                        </div>
                        <div
                            class:has-text-weight-semibold
                            class:is-family-monospace
                            style:font-size="0.95rem"
                            style:line-height="1.1"
                        >
                            {format!("{score:.0} / {max_score:.0}")}
                        </div>
                    </div>
                    <progress
                        class="progress is-info"
                        style:width="4.5rem"
                        style:margin="0"
                        value=score
                        max=max_score
                    />
                </div>
            </ShowLet>
            <div class="is-flex-grow-1" />
            <Show when=move || backend::for_lang(language.read().deref()).has_dynamic_io()>
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
                    on:click=do_stop
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
                    <Icon class:icon class:is-left class:mr-1 icon=icondata::AiCaretRightFilled />
                    {t!(i18n, run)}
                </button>
            </Show>
            <Show when=move || contest_api::get().is_some()>
                <button
                    class:has-icons-left
                    class:button
                    class:is-info
                    class:mr-1
                    style:width="8em"
                    disabled=disable_submit
                    on:click=move |_| {
                        if !disable_submit.get() {
                            do_submit.run(())
                        }
                    }
                >
                    <Icon class:icon class:is-left class:mr-1 icon=icondata::AiSendOutlined />
                    {t!(i18n, submit)}
                </button>
            </Show>
        </div>
    };

    let disable_input_editor = Memo::new(move |_| {
        is_running.get()
            || get_input_mode(input_mode.get(), language.read().deref())
                == InputMode::FullInteractive
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
fn LoadingView() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div
            class:is-flex
            class:is-flex-direction-row
            class:is-align-items-center
            class:is-justify-content-center
            style:height="100dvh"
        >
            <span class="loader" />
            <p class="is-size-4 ml-2">{t!(i18n, loading)}</p>
        </div>
    }
}

#[component]
fn ConfigAndBackendProvider(mut children: ChildrenFnMut) -> impl IntoView {
    let config = LocalResource::new(|| async {
        let res = Request::get("config.json").send().await.unwrap();
        assert!(res.ok(), "could not load config: {}", res.status());
        let config: Config = res.json().await.unwrap();

        backend::register_backend(WorkerBackend::new().await);
        backend::register_backend(JsBackend::new().await);
        if let Some(remote_eval) = &config.remote_eval {
            backend::register_backend(RemoteBackend::new(remote_eval.clone()).await.unwrap());
        }

        contest_api::init(&config).await;

        config
    });

    move || match config.get() {
        Some(config) => {
            provide_context::<Config>(config);
            children()
        }
        None => view! { <LoadingView /> }.into_any(),
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
