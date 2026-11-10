#![allow(deprecated)]
leptos_i18n::load_locales!();

#[cfg(test)]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;

use anyhow::{Context, Result};
use common::{
    ExecConfig, File, WorkerExecRequest, WorkerExecResponse, WorkerExecStatus, WorkerLSRequest,
    WorkerLSResponse, WorkerRequest, WorkerResponse, init_logging,
};
use futures_channel::mpsc::{UnboundedSender, unbounded};
use gloo_net::http::Request;
use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use leptos::task::{spawn_local, spawn_local_scoped};
use leptos_i18n::context::CookieOptions;
use leptos_use::{use_document, use_event_listener, use_window};
use tracing::{info, warn};

mod backend;
mod config;
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
use crate::config::Config;
use crate::contest_api::{ContestPhase, ContestStatus, SubmitStatus, format_countdown};
use crate::editor_dir::{EditorDirController, select_best_code_file};
use crate::editor_view::EditorView;
use crate::enum_select::EnumSelect;
use crate::i18n::*;
use crate::settings::{InputMode, Settings, SettingsProvider, use_settings};
use crate::status_view::StatusView;
use crate::util::{Icon, check_response, get_input_mode};
use crate::workspace::{
    WorkspaceConfig, WorkspaceSelector, ensure_contest_workspaces, ensure_default_workspace,
    get_saved_workspace, primary_extension_for_lang, set_saved_workspace,
};

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
    let dismissed = RwSignal::new(false);
    let ok = LocalResource::new(move || async move {
        if settings.persist_storage.get() {
            dismissed.set(false);
            common::opfs::persist().await
        } else {
            true
        }
    });

    view! {
        <Show when=move || ok.get() == Some(false) && !dismissed.get()>
            <div class="alert alert-warning shadow-lg py-2 px-3 text-xs fixed bottom-3 left-3 z-[900] w-auto max-w-sm flex items-center gap-2">
                <Icon attr:class="w-4 h-4 shrink-0" icon=icondata::BiErrorSolid />
                <span>{t!(i18n, storage_denied)}</span>
                <button
                    type="button"
                    class="btn btn-ghost btn-xs btn-circle shrink-0"
                    on:click=move |_| dismissed.set(true)
                >
                    <Icon attr:class="w-3.5 h-3.5" icon=icondata::IoClose />
                </button>
            </div>
        </Show>
    }
}

fn resolve_execution_files(
    lang_name: &str,
    task_name: Option<&str>,
    open_file: Option<&str>,
    all_files: Vec<(String, Vec<u8>)>,
) -> (Option<String>, Vec<(String, Vec<u8>)>) {
    let all_langs = backend::languages();
    let lang_exts = all_langs
        .iter()
        .find(|l| l.name == lang_name)
        .map(|l| l.extensions.as_slice())
        .unwrap_or(&[]);
    let other_lang_exts: Vec<&str> = all_langs
        .iter()
        .filter(|l| l.name != lang_name)
        .flat_map(|l| l.extensions.iter().map(String::as_str))
        .filter(|ext| !lang_exts.iter().any(|e| e == ext))
        .collect();

    let matches_lang = |name: &str| {
        let ext = name.rsplit('.').next().unwrap_or("");
        lang_exts.iter().any(|e| e == ext)
    };
    let is_grader = |name: &str| {
        let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
        stem == "grader"
    };

    let filtered_files: Vec<(String, Vec<u8>)> = all_files
        .into_iter()
        .filter(|(name, _)| {
            let ext = name.rsplit('.').next().unwrap_or("");
            !other_lang_exts.contains(&ext)
        })
        .collect();

    let names: Vec<String> = filtered_files.iter().map(|(n, _)| n.clone()).collect();
    let has_non_grader_lang_file = names.iter().any(|n| matches_lang(n) && !is_grader(n));

    let primary_file = open_file
        .filter(|open| {
            names.iter().any(|n| n == open)
                && matches_lang(open)
                && (!is_grader(open) || !has_non_grader_lang_file)
        })
        .map(ToOwned::to_owned)
        .or_else(|| select_best_code_file(&names, lang_name, task_name))
        .or_else(|| {
            open_file
                .filter(|open| names.iter().any(|n| n == open))
                .map(ToOwned::to_owned)
        });

    (primary_file, filtered_files)
}

#[component]
fn App(initial_workspace: Option<String>) -> impl IntoView {
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

    let settings = use_settings();
    let SettingsProvider {
        input_mode,
        mem_limit,
        time_limit,
        default_language,
        ..
    } = settings;

    backend::set_callback(Arc::new({
        let ls_sender = ls_sender.clone();
        move |msg| handle_message(msg, state, fetching_compiler_progress, &ls_sender).unwrap()
    }));

    let workspace = RwSignal::new(initial_workspace);
    let workspaces_revision = RwSignal::new(0u64);
    let contest_status = RwSignal::new(None::<ContestStatus>);
    let checking_contest_status = RwSignal::new(false);
    let now_ms = RwSignal::new(js_sys::Date::now() as i64);

    let refresh_contest_status = Callback::new(move |()| {
        let Some(api) = contest_api::get() else {
            return;
        };
        if checking_contest_status.get_untracked() {
            return;
        }
        checking_contest_status.set(true);
        spawn_local(async move {
            match api.contest_status().await {
                Ok(status) => {
                    now_ms.set(js_sys::Date::now() as i64);
                    contest_status.set(Some(status));
                }
                Err(err) => {
                    warn!("Failed to query contest status: {err:?}");
                }
            }
            checking_contest_status.set(false);
        });
    });

    if contest_api::get().is_some() {
        refresh_contest_status.run(());

        spawn_local(async move {
            loop {
                TimeoutFuture::new(1000).await;
                now_ms.set(js_sys::Date::now() as i64);
            }
        });

        spawn_local(async move {
            loop {
                let delay_ms = contest_status.with_untracked(|s| match s {
                    Some(status)
                        if status.logged_in
                            && status.effective_phase(now_ms.get_untracked())
                                == ContestPhase::Running =>
                    {
                        30_000
                    }
                    _ => 10_000,
                });
                TimeoutFuture::new(delay_ms).await;
                refresh_contest_status.run(());
            }
        });

        let _ = use_event_listener(use_window(), leptos::ev::focus, move |_| {
            refresh_contest_status.run(());
        });
        let _ = use_event_listener(use_document(), leptos::ev::visibilitychange, move |_| {
            if !gloo_utils::document().hidden() {
                refresh_contest_status.run(());
            }
        });
    }

    Effect::new(move |_| {
        let current = workspace.get();
        set_saved_workspace(current.as_deref());
    });
    let workspace_config = LocalResource::new(move || {
        let workspace = workspace.get();
        async move {
            let ws = workspace?;
            WorkspaceConfig::load(&ws).await.ok()
        }
    });
    let task_score = LocalResource::new(move || {
        let workspace_config = workspace_config.get();
        let _ = workspaces_revision.get();
        async move {
            let config = workspace_config.flatten()?;
            let task = config.task?;
            let api = contest_api::get()?;
            api.task_score(&task).await.ok()
        }
    });

    let effective_contest_phase = Memo::new(move |_| {
        let now = now_ms.get();
        contest_status
            .get()
            .map(|status| (status.logged_in, status.effective_phase(now)))
    });

    {
        let app_config = expect_context::<Config>();
        Effect::new(move |prev: Option<Option<(bool, ContestPhase)>>| {
            let current = effective_contest_phase.get();
            if let Some(Some((_, prev_phase))) = prev
                && let Some((_, new_phase)) = current
                && prev_phase != new_phase
            {
                refresh_contest_status.run(());
            }
            let became_active = prev != Some(Some((true, ContestPhase::Running)))
                && current == Some((true, ContestPhase::Running));
            if became_active {
                workspaces_revision.update(|r| *r += 1);
                if app_config.auto_init_contest_tasks
                    && let Some(api) = contest_api::get()
                {
                    let lang = default_language
                        .get_untracked()
                        .or_else(|| backend::languages().first().map(|l| l.name.clone()))
                        .unwrap_or_else(|| "C++".to_owned());
                    spawn_local(async move {
                        if let Err(err) = ensure_contest_workspaces(&api, &lang).await {
                            warn!("Failed to auto-init contest workspaces: {err:?}");
                            return;
                        }
                        workspaces_revision.update(|r| *r += 1);
                        if workspace.get_untracked().is_none() {
                            let ws_dir = common::opfs::open_dir("workspace", true).await;
                            let mut entries = ws_dir.list_entries().await;
                            entries.sort();
                            let chosen = get_saved_workspace()
                                .filter(|s| entries.contains(s))
                                .or_else(|| entries.into_iter().next());
                            if chosen.is_some() {
                                workspace.set(chosen);
                            }
                        }
                    });
                }
            }
            current
        });
    }

    let contest_ready = Signal::derive(move || match contest_api::get() {
        None => true,
        Some(_) => effective_contest_phase.get() == Some((true, ContestPhase::Running)),
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
        workspace_config
            .get()
            .flatten()
            .map(|cfg| cfg.language)
            .filter(|l| !l.is_empty())
            .or_else(|| {
                code.open_filename().get().and_then(|f| {
                    let ext = f.split('.').next_back().unwrap_or("");
                    backend::languages()
                        .iter()
                        .find(|lang| lang.extensions.iter().any(|e| e == ext))
                        .map(|lang| lang.name.clone())
                })
            })
            .or_else(|| default_language.get())
            .or(old.cloned())
            .unwrap_or_else(|| "C++".to_owned())
    });

    // TODO(virv): remove this
    let send_worker_message = move |msg: WorkerRequest| {
        backend::for_lang(&language.get_untracked()).send_message(msg);
    };

    let disable_start = Memo::new(move |_| {
        state.with(|s| !s.can_start())
            || workspace.get().is_none()
            || code.open_filename().get().is_none()
    });
    let disable_stop = Memo::new(move |_| state.with(|s| !s.can_stop()));
    let is_running = Memo::new(move |_| state.with(|s| s.is_running()));

    {
        let ls_sender = ls_sender.clone();
        Effect::new(move |old: Option<(String, Option<String>, u64)>| {
            let lang = language.get();
            let code_dir = code.dir().get();
            let revision = code.files_revision().get();
            info!("Requesting language server for {lang:?}");
            if let Some((old_lang, _, _)) = &old {
                backend::for_lang(old_lang).send_message(WorkerLSRequest::Stop.into());
            }
            ls_sender.unbounded_send(WorkerLSResponse::Stopped).unwrap();
            if let Some(code_dir) = &code_dir {
                state.update(|s| {
                    s.ls = StateLS::Requested;
                });
                send_worker_message(
                    WorkerLSRequest::Start {
                        language: lang.clone(),
                        code_dir: code_dir.clone(),
                    }
                    .into(),
                );
            } else {
                state.update(|s| s.ls = StateLS::Ready);
            }
            (lang, code_dir, revision)
        });
    }

    let do_run = Callback::new(move |()| {
        let Some(ws) = workspace.get_untracked() else {
            return;
        };
        let open_file = code
            .open_filename()
            .get_untracked()
            .and_then(|p| p.split('/').next_back().map(ToOwned::to_owned));

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

            let task_name = WorkspaceConfig::load(&ws)
                .await
                .ok()
                .and_then(|cfg| cfg.task);
            let mut raw_files = Vec::new();
            let dir = common::opfs::open_dir(&format!("workspace/{ws}/code"), false).await;
            for name in dir.list_entries().await {
                let file = dir.open_file(&name, false).await;
                let content = file.read().await;
                raw_files.push((name, content));
            }

            let lng = language.get_untracked();
            let (primary_file, filtered_files) = resolve_execution_files(
                &lng,
                task_name.as_deref(),
                open_file.as_deref(),
                raw_files,
            );
            let Some(primary_file) = primary_file else {
                state.update(|s| {
                    s.exec = StateExec::Complete {
                        outcome: Outcome::default(),
                        error: Some(format!("No source file found for {lng}")),
                    };
                });
                return;
            };
            let files = filtered_files
                .into_iter()
                .map(|(name, content)| File { name, content })
                .collect();

            info!("Requesting execution");
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
            || code.open_filename().get().is_none()
            || workspace_config
                .get()
                .map(|cfg| cfg.and_then(|cfg| cfg.task).is_none())
                .unwrap_or(false)
            || effective_contest_phase
                .get()
                .is_some_and(|(logged_in, phase)| !logged_in || phase != ContestPhase::Running)
    });

    let submit_title = Signal::derive(move || match effective_contest_phase.get() {
        Some((false, _)) => t_string!(i18n, contest_not_logged_in).to_string(),
        Some((true, ContestPhase::NotStarted)) => t_string!(i18n, contest_not_started).to_string(),
        Some((true, ContestPhase::Ended)) => t_string!(i18n, contest_ended).to_string(),
        Some((true, ContestPhase::NotOpen)) => t_string!(i18n, contest_not_open).to_string(),
        _ => t_string!(i18n, submit).to_string(),
    });

    let app_owner = Owner::current();
    let do_submit = Callback::new(move |_| {
        let Some(ws) = workspace.get_untracked() else {
            warn!("asked to submit without a workspace");
            return;
        };
        let open_file = code
            .open_filename()
            .get_untracked()
            .and_then(|p| p.split('/').next_back().map(ToOwned::to_owned));
        let Some(api) = contest_api::get() else {
            warn!("asked to submit without contest api");
            return;
        };
        state.update(|s| {
            s.submit = StateSubmit::Submitting(None);
        });

        let fut = async move {
            code.wait_sync().await;

            let Ok(config) = WorkspaceConfig::load(&ws).await else {
                state.update(|s| {
                    s.submit = StateSubmit::Error("Failed to load workspace config".into())
                });
                return;
            };

            let mut raw_files = Vec::new();
            let dir = common::opfs::open_dir(&format!("workspace/{ws}/code"), false).await;
            for name in dir.list_entries().await {
                let file = dir.open_file(&name, false).await;
                let content = file.read().await;
                raw_files.push((name, content));
            }

            let lng = language.get_untracked();
            let (primary_file, files) = resolve_execution_files(
                &lng,
                config.task.as_deref(),
                open_file.as_deref(),
                raw_files,
            );
            let Some(primary_file) = primary_file else {
                state.update(|s| {
                    s.submit = StateSubmit::Error(format!("No source file found for {lng}"))
                });
                return;
            };

            let res = api
                .submit(
                    config
                        .task
                        .as_deref()
                        .expect("submit without connected task"),
                    &lng,
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
        };
        if let Some(owner) = &app_owner {
            owner.with(|| spawn_local_scoped(fut));
        } else {
            spawn_local_scoped(fut);
        }
    });

    let exec_stage_label = Signal::derive(move || match &state.read().exec {
        StateExec::Processing { stopping: true, .. } => {
            Some(t_string!(i18n, stopping_execution).to_string())
        }
        StateExec::Processing {
            status: Some(WorkerExecStatus::FetchingCompiler),
            ..
        } => Some(t_string!(i18n, downloading_runtime).to_string()),
        StateExec::Processing {
            status: Some(WorkerExecStatus::Compiling),
            ..
        } => Some(t_string!(i18n, compiling).to_string()),
        StateExec::Processing {
            status: None | Some(WorkerExecStatus::Running),
            ..
        } => Some(t_string!(i18n, executing).to_string()),
        _ => None,
    });

    let navbar = view! {
        <div class="flex flex-row items-center gap-2 px-3 py-1.5 bg-base-100 border-b border-base-300 min-h-12 shrink-0">
            <Settings />
            <WorkspaceSelector
                active=workspace
                active_language=language
                readonly=is_running
                workspaces_revision=workspaces_revision
                contest_ready=contest_ready
            />
            <ShowLet some=move || task_score.get().flatten() let:((score, max_score))>
                <div class="hidden sm:flex items-center gap-2.5 px-3 py-1 bg-base-200/80 border border-base-300 rounded-lg">
                    <div class="flex flex-col leading-none">
                        <span class="uppercase font-semibold text-[10px] tracking-wider text-base-content/60">
                            {t!(i18n, task_score)}
                        </span>
                        <span class="font-semibold font-mono text-xs mt-0.5">
                            {format!("{score:.0} / {max_score:.0}")}
                        </span>
                    </div>
                    <progress
                        class=move || {
                            if max_score > 0.0 && score >= max_score {
                                "progress progress-success w-16 h-2"
                            } else {
                                "progress progress-info w-16 h-2"
                            }
                        }
                        value=score
                        max=max_score
                    />
                </div>
            </ShowLet>
            <ContestTimeBadge status=contest_status now_ms=now_ms />
            <div class="flex-1" />
            <ShowLet some=exec_stage_label let:label>
                <div class="hidden md:flex items-center gap-1.5 text-xs font-medium text-base-content/70 px-2">
                    <span class="loading loading-spinner loading-xs text-primary" />
                    <span>{label}</span>
                </div>
            </ShowLet>
            <Show when=move || backend::for_lang(language.read().deref()).has_dynamic_io()>
                <EnumSelect value=(input_mode, SignalSetter::map(move |m| settings.set_input_mode(m))) />
            </Show>
            <Show when=move || is_running.get()>
                <button
                    type="button"
                    class="btn btn-error btn-sm gap-1.5 min-w-24"
                    disabled=disable_stop
                    on:click=do_stop
                >
                    <Icon attr:class="w-4 h-4" icon=icondata::AiCloseOutlined />
                    {t!(i18n, stop)}
                </button>
            </Show>
            <Show when=move || !is_running.get()>
                <button
                    type="button"
                    class="btn btn-success btn-sm gap-1.5 min-w-24"
                    title=move || format!("{} (Ctrl+Enter)", t_string!(i18n, run))
                    disabled=disable_start
                    on:click=move |_| {
                        if !disable_start.get() {
                            do_run.run(())
                        }
                    }
                >
                    <Icon attr:class="w-4 h-4" icon=icondata::AiCaretRightFilled />
                    {t!(i18n, run)}
                </button>
            </Show>
            <Show when=move || contest_api::get().is_some()>
                <button
                    type="button"
                    class="btn btn-info btn-sm gap-1.5 min-w-24"
                    title=submit_title
                    disabled=disable_submit
                    on:click=move |_| {
                        if !disable_submit.get() {
                            do_submit.run(())
                        }
                    }
                >
                    <Icon attr:class="w-4 h-4" icon=icondata::AiSendOutlined />
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
        <div class="flex flex-col h-dvh w-screen overflow-hidden bg-base-200">
            {navbar}
            <ContestLoginBanner
                status=contest_status
                checking=checking_contest_status
                on_retry=refresh_contest_status
            />
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
                state=state
            />
        </div>
    }
}

#[component]
fn ContestLoginBanner(
    #[prop(into)] status: Signal<Option<ContestStatus>>,
    #[prop(into)] checking: Signal<bool>,
    on_retry: Callback<()>,
) -> impl IntoView {
    let i18n = use_i18n();
    let unauthenticated = Signal::derive(move || status.get().filter(|s| !s.logged_in));

    view! {
        <ShowLet some=unauthenticated let:status>
            <div class="flex flex-wrap items-center justify-between gap-2 px-3 py-1.5 bg-warning/15 border-b border-warning/30 text-xs shrink-0">
                <div class="flex items-center gap-2 font-medium text-base-content">
                    <Icon attr:class="w-4 h-4 text-warning shrink-0" icon=icondata::BiErrorSolid />
                    <span>{t!(i18n, contest_not_logged_in)}</span>
                </div>
                <div class="flex items-center gap-1.5">
                    <a
                        href=status.login_url
                        target="_blank"
                        rel="noopener noreferrer"
                        class="btn btn-warning btn-xs gap-1"
                    >
                        {t!(i18n, contest_login_action)}
                        <Icon attr:class="w-3.5 h-3.5" icon=icondata::BiLinkExternalRegular />
                    </a>
                    <button
                        type="button"
                        class="btn btn-ghost btn-xs gap-1"
                        disabled=move || checking.get()
                        on:click=move |_| on_retry.run(())
                    >
                        <Icon
                            attr:class=move || {
                                if checking.get() { "w-3.5 h-3.5 animate-spin" } else { "w-3.5 h-3.5" }
                            }
                            icon=icondata::LuRotateCcw
                        />
                        {t!(i18n, contest_retry_login)}
                    </button>
                </div>
            </div>
        </ShowLet>
    }
}

#[component]
fn ContestTimeBadge(
    #[prop(into)] status: Signal<Option<ContestStatus>>,
    #[prop(into)] now_ms: Signal<i64>,
) -> impl IntoView {
    let i18n = use_i18n();
    let badge_state = Memo::new(move |_| {
        let status = status.get()?;
        let now = now_ms.get();
        let phase = status.effective_phase(now);
        let countdown = status.countdown_seconds(now);
        match (phase, countdown, status.logged_in) {
            (ContestPhase::Running, None, _) => None,
            (_, None, false) => None,
            _ => Some((phase, countdown)),
        }
    });

    view! {
        <ShowLet some=move || badge_state.get() let:((phase, countdown))>
            <div
                class=move || {
                    let base = "hidden sm:inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg border text-xs shrink-0";
                    match (phase, countdown) {
                        (ContestPhase::Running, Some(secs)) if secs <= 300 => {
                            format!("{base} bg-error/15 border-error/40 text-error")
                        }
                        (ContestPhase::Running, Some(secs)) if secs <= 900 => {
                            format!("{base} bg-warning/15 border-warning/40 text-base-content")
                        }
                        (ContestPhase::Ended, _) => {
                            format!("{base} bg-error/10 border-error/30 text-error")
                        }
                        (ContestPhase::NotStarted | ContestPhase::NotOpen, _) => {
                            format!("{base} bg-base-200/80 border-base-300 text-base-content/80")
                        }
                        _ => format!("{base} bg-base-200/80 border-base-300 text-base-content"),
                    }
                }
                title=move || match phase {
                    ContestPhase::Running => t_string!(i18n, contest_time_remaining).to_string(),
                    ContestPhase::NotStarted => t_string!(i18n, contest_not_started).to_string(),
                    ContestPhase::Ended => t_string!(i18n, contest_ended).to_string(),
                    ContestPhase::NotOpen => t_string!(i18n, contest_not_open).to_string(),
                }
            >
                <Icon attr:class="w-3.5 h-3.5 shrink-0" icon=icondata::BiTimeFiveRegular />
                {match (phase, countdown) {
                    (ContestPhase::NotStarted, Some(secs)) => {
                        view! {
                            <span class="text-base-content/70">{t!(i18n, contest_starts_in)}</span>
                            <span class="font-mono font-semibold">{format_countdown(secs)}</span>
                        }
                            .into_any()
                    }
                    (ContestPhase::NotStarted, None) => {
                        view! { <span class="font-medium">{t!(i18n, contest_not_started)}</span> }
                            .into_any()
                    }
                    (ContestPhase::Running, Some(secs)) => {
                        view! { <span class="font-mono font-semibold">{format_countdown(secs)}</span> }
                            .into_any()
                    }
                    (ContestPhase::Ended, _) => {
                        view! { <span class="font-medium">{t!(i18n, contest_ended)}</span> }
                            .into_any()
                    }
                    (ContestPhase::NotOpen | ContestPhase::Running, _) => {
                        view! { <span class="font-medium">{t!(i18n, contest_not_open)}</span> }
                            .into_any()
                    }
                }}
            </div>
        </ShowLet>
    }
}

#[component]
fn LoadingView() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="flex flex-row items-center justify-center h-dvh gap-3 bg-base-200">
            <span class="loading loading-spinner loading-lg text-primary" />
            <p class="text-lg font-medium">{t!(i18n, loading)}</p>
        </div>
    }
}

#[component]
fn StartupErrorView(#[prop(into)] err: String) -> impl IntoView {
    view! {
        <div class="flex items-center justify-center h-dvh px-4 bg-base-200">
            <div class="alert alert-error shadow-lg max-w-3xl w-full flex flex-col items-start gap-2">
                <div class="flex items-center gap-2 font-bold text-base">
                    <Icon attr:class="w-5 h-5" icon=icondata::BiErrorCircleSolid />
                    <span>"Startup Error"</span>
                </div>
                <pre class="text-xs font-mono whitespace-pre-wrap break-all w-full bg-base-100/20 p-3 rounded">
                    {err}
                </pre>
            </div>
        </div>
    }
}

async fn initialize_initial_workspaces(config: &Config, default_lang: &str) -> Option<String> {
    if let Some(api) = contest_api::get() {
        if config.auto_init_contest_tasks
            && let Err(err) = ensure_contest_workspaces(&api, default_lang).await
        {
            warn!("Failed to auto-init contest workspaces: {err:?}");
        }
    } else {
        let ws_dir = common::opfs::open_dir("workspace", true).await;
        if ws_dir.list_entries().await.is_empty() {
            let ext = primary_extension_for_lang(default_lang);
            let default_ws = config.local_workspace(default_lang, &ext);
            ensure_default_workspace(&default_ws, default_lang).await;
        }
    }

    let ws_dir = common::opfs::open_dir("workspace", true).await;
    let mut entries = ws_dir.list_entries().await;
    entries.sort();
    let saved = get_saved_workspace();
    saved
        .filter(|s| entries.contains(s))
        .or_else(|| entries.into_iter().next())
}

#[component]
fn ChooseDefaultLanguageView(on_complete: Callback<Option<String>>) -> impl IntoView {
    let i18n = use_i18n();
    let config = expect_context::<Config>();
    let settings = use_settings();
    let langs = backend::languages();
    let initial_choice = langs
        .first()
        .map(|l| l.name.clone())
        .unwrap_or_else(|| "C++".to_owned());
    let selected = RwSignal::new(initial_choice);
    let initializing = RwSignal::new(false);

    let confirm = Callback::new(move |()| {
        if initializing.get_untracked() {
            return;
        }
        let chosen = selected.get_untracked();
        if chosen.is_empty() {
            return;
        }
        initializing.set(true);
        let config = config.clone();
        spawn_local(async move {
            let initial_ws = initialize_initial_workspaces(&config, &chosen).await;
            settings.set_default_language(chosen);
            on_complete.run(initial_ws);
        });
    });

    view! {
        <div class="flex items-center justify-center min-h-dvh p-4 bg-base-200">
            <div class="card bg-base-100 border border-base-300 shadow-xl w-full max-w-md">
                <div class="card-body gap-4">
                    <div class="flex flex-col gap-1">
                        <h2 class="card-title text-lg font-bold">
                            {t!(i18n, choose_default_language_title)}
                        </h2>
                        <p class="text-sm text-base-content/70">
                            {t!(i18n, choose_default_language_desc)}
                        </p>
                    </div>

                    <div class="flex flex-col gap-2">
                        <For each=move || backend::languages() key=|l| l.name.clone() let:lang>
                            {
                                let lang_name = lang.name.clone();
                                let select_name = lang.name.clone();
                                let dbl_name = lang.name.clone();
                                let exts = lang
                                    .extensions
                                    .iter()
                                    .map(|e| format!(".{e}"))
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                view! {
                                    <button
                                        type="button"
                                        disabled=move || initializing.get()
                                        class=move || {
                                            if selected.get() == lang_name {
                                                "flex items-center justify-between px-4 py-3 rounded-lg border-2 border-primary bg-primary/10 text-left transition-colors cursor-pointer"
                                            } else {
                                                "flex items-center justify-between px-4 py-3 rounded-lg border border-base-300 hover:bg-base-200 text-left transition-colors cursor-pointer"
                                            }
                                        }
                                        on:click=move |_| selected.set(select_name.clone())
                                        on:dblclick=move |_| {
                                            selected.set(dbl_name.clone());
                                            confirm.run(());
                                        }
                                    >
                                        <span class="font-medium text-sm">{lang.name}</span>
                                        <span class="badge badge-ghost badge-sm font-mono">{exts}</span>
                                    </button>
                                }
                            }
                        </For>
                    </div>

                    <div class="card-actions justify-end pt-2">
                        <button
                            type="button"
                            class="btn btn-primary w-full sm:w-auto min-w-32"
                            disabled=move || initializing.get()
                            on:click=move |_| confirm.run(())
                        >
                            <Show when=move || initializing.get()>
                                <span class="loading loading-spinner loading-xs" />
                            </Show>
                            {t!(i18n, continue_btn)}
                        </button>
                    </div>
                </div>
            </div>
        </div>
    }
}

#[component]
fn WorkspaceBootstrap() -> impl IntoView {
    let config = expect_context::<Config>();
    let settings = use_settings();
    let ready_workspace = RwSignal::new(None::<Option<String>>);

    if let Some(lang) = settings.default_language.get_untracked() {
        let config = config.clone();
        spawn_local(async move {
            let initial_ws = initialize_initial_workspaces(&config, &lang).await;
            ready_workspace.set(Some(initial_ws));
        });
    }

    move || match ready_workspace.get() {
        Some(initial_ws) => view! { <App initial_workspace=initial_ws /> }.into_any(),
        None if settings.default_language.get().is_some() => view! { <LoadingView /> }.into_any(),
        None => view! {
            <ChooseDefaultLanguageView on_complete=Callback::new(move |initial_ws| {
                ready_workspace.set(Some(initial_ws));
            }) />
        }
        .into_any(),
    }
}

#[component]
fn ConfigAndBackendProvider() -> impl IntoView {
    let i18n = use_i18n();
    let config = LocalResource::new(|| async {
        let config = async {
            let res = Request::get("config.json")
                .send()
                .await
                .context("Failed to fetch config.json")?;
            check_response(&res, "Failed to load config.json").await?;
            let config: Config = res.json().await.context("Failed to parse config.json")?;

            config.validate()?;

            backend::register_backend(WorkerBackend::new().await);
            backend::register_backend(JsBackend::new().await);
            if let Some(remote_eval) = &config.remote_eval {
                let backend = RemoteBackend::new(remote_eval.clone())
                    .await
                    .with_context(|| {
                        format!("Failed to initialize remote backend at {remote_eval}")
                    })?;
                backend::register_backend(backend);
            }

            if let Some(lang) = &config.default_language {
                anyhow::ensure!(
                    backend::languages().iter().any(|l| &l.name == lang),
                    "Unsupported default_language in config.json: {lang}"
                );
            }

            contest_api::init(config.contest.as_ref()).await;

            Ok::<_, anyhow::Error>(config)
        }
        .await;

        config.map_err(|err| format!("{err:?}"))
    });

    move || match config.get() {
        Some(Ok(config)) => {
            if let Some(title) = &config.title {
                gloo_utils::document().set_title(title);
            }
            let initial_locale = config
                .default_locale
                .unwrap_or_else(|| i18n.get_locale_untracked());
            SettingsProvider::install(
                config.default_settings.clone(),
                config.default_language.clone(),
            );
            provide_context::<Config>(config);
            view! {
                <I18nSubContextProvider
                    initial_locale=initial_locale
                    cookie_name=LOCALE_COOKIE
                >
                    <WorkspaceBootstrap />
                </I18nSubContextProvider>
            }
            .into_any()
        }
        Some(Err(err)) => view! { <StartupErrorView err=err.to_string() /> }.into_any(),
        None => view! { <LoadingView /> }.into_any(),
    }
}

const LOCALE_COOKIE: &str = "i18n_pref_locale";

fn main() {
    init_logging();

    mount_to_body(move || {
        let cookie_options = CookieOptions::<Locale>::default().readonly(true);
        view! {
            <I18nContextProvider cookie_name=LOCALE_COOKIE cookie_options=cookie_options>
                <ConfigAndBackendProvider />
            </I18nContextProvider>
        }
    })
}
