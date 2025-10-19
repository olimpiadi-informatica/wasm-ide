leptos_i18n::load_locales!();

use std::collections::HashMap;
use std::collections::HashSet;
use std::ops::DerefMut;

use anyhow::Result;
use async_channel::{unbounded, Sender};
use common::{
    init_logging, File, Language, WorkerExecRequest, WorkerExecResponse, WorkerExecStatus,
    WorkerLSRequest, WorkerLSResponse, WorkerRequest, WorkerResponse,
};
use leptos::{context::Provider, prelude::*};
use send_wrapper::SendWrapper;
use serde::{Deserialize, Serialize};
use thaw::{
    Button, ButtonType, ConfigProvider, Flex, FlexAlign, Grid, GridItem, Input, Layout,
    LayoutHeader, LayoutPosition, MessageBar, MessageBarBody, MessageBarIntent,
};
use tracing::{debug, info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{MessageEvent, Worker, WorkerOptions, WorkerType};

use i18n::*;

mod editor;
mod enum_select;
mod output;
mod settings;
mod status_view;
mod theme;
mod util;

use crate::editor::{Editor, EditorText};
use crate::enum_select::EnumSelect;
use crate::output::OutputView;
use crate::settings::Settings;
use crate::status_view::StatusView;
use crate::util::{load, save};

#[derive(PartialEq, Eq, Clone, Copy, Hash, Debug, Serialize, Deserialize)]
pub enum KeyboardMode {
    Standard,
    Vim,
    Emacs,
}

#[derive(PartialEq, Eq, Clone, Copy, Hash, Debug, Serialize, Deserialize)]
pub enum InputMode {
    Batch,
    MixedInteractive,
    FullInteractive,
}

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
        <Show when=move || large_files.with(|lf| !lf.0.is_empty())>
            <MessageBar class="storage-error-view" intent=MessageBarIntent::Warning>
                <MessageBarBody>{t!(i18n, files_too_big)}</MessageBarBody>
            </MessageBar>
        </Show>
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

fn input_mode_string(locale: Locale, input_mode: InputMode) -> String {
    match input_mode {
        InputMode::Batch => td_display!(locale, batch_input),
        InputMode::MixedInteractive => td_display!(locale, mixed_interactive_input),
        InputMode::FullInteractive => td_display!(locale, full_interactive_input),
    }
    .to_string()
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

    let disable_start = Signal::from(disable_start);
    let disable_stop = Signal::from(disable_stop);

    let lang = RwSignal::new(load("language").unwrap_or(Language::CPP));
    let lang_options = [Language::CPP, Language::C, Language::Python]
        .into_iter()
        .map(|lng| (lng, Signal::stored(lng.into())))
        .collect::<Vec<_>>();

    {
        let worker_ready = Memo::new(move |_| matches!(*state.read(), RunState::Ready { .. }));

        let send_worker_message = send_worker_message.clone();
        let ls_sender = ls_sender.clone();
        Effect::new(move |_| {
            if !worker_ready.get() {
                return;
            }

            let lang = lang.get();
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

    let input_mode = RwSignal::new(load("input_mode").unwrap_or(InputMode::Batch));
    let input_options = [
        InputMode::Batch,
        InputMode::MixedInteractive,
        InputMode::FullInteractive,
    ]
    .into_iter()
    .map(|mode| {
        (
            mode,
            Signal::derive(move || input_mode_string(i18n.get_locale(), mode)),
        )
    })
    .collect::<Vec<_>>();
    Effect::new(move |_| save("input_mode", &input_mode.get()));

    let do_run = {
        let send_worker_message = send_worker_message.clone();
        move || {
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
                let lng = lang.get_untracked();
                send_worker_message(
                    WorkerExecRequest::CompileAndRun {
                        files: vec![File {
                            name: format!("solution.{}", lng.ext()),
                            content: code,
                        }],
                        language: lng,
                        input,
                    }
                    .into(),
                );
                if let Some(addn_msg) = addn_msg {
                    send_worker_message(addn_msg.into());
                }
            });
        }
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

    Effect::new(move |_| save("language", &lang.get()));

    let kb_mode = RwSignal::new(load("kb_mode").unwrap_or(KeyboardMode::Standard));
    Effect::new(move |_| save("kb_mode", &kb_mode.get()));

    let navbar = {
        let do_run = do_run.clone();
        view! {
            <Flex align=FlexAlign::Center style="padding: 0 20px; height: 64px;">
                <Settings kb_mode />
                <EnumSelect
                    class="language-selector"
                    value=(lang.into(), lang.into())
                    options=lang_options
                />
                {move || match is_running.get() {
                    true => {
                        let do_stop = do_stop.clone();
                        view! {
                            <Button
                                class="red"
                                loading=disable_stop
                                icon=icondata::AiCloseOutlined
                                on_click=do_stop
                            >
                                {t!(i18n, stop)}
                            </Button>
                        }
                    }
                    false => {
                        let do_run = do_run.clone();
                        view! {
                            <Button
                                class="green"
                                loading=disable_start
                                icon=icondata::AiCaretRightFilled
                                on_click=move |_| do_run()
                            >
                                {t!(i18n, run)}
                            </Button>
                        }
                    }
                }}
                <EnumSelect value=(input_mode.into(), input_mode.into()) options=input_options />
            </Flex>
        }
    };

    let additional_input = RwSignal::new(String::from(""));

    let add_input = {
        let send_worker_message = send_worker_message.clone();
        move || {
            let mut extra = additional_input.get_untracked();
            if extra.is_empty() {
                return;
            }
            additional_input.set(String::new());
            let cur_stdin = stdin.with_untracked(|x| x.text().clone());
            if !cur_stdin.is_empty() && !cur_stdin.ends_with('\n') {
                extra = format!("\n{extra}");
            }
            if !extra.ends_with('\n') {
                extra = format!("{extra}\n");
            }
            stdin.set(EditorText::from_str(&(cur_stdin + &extra)));
            send_worker_message(WorkerExecRequest::StdinChunk(extra.into_bytes()).into());
        }
    };

    let additional_input_string =
        Signal::derive(move || t_display!(i18n, additional_input).to_string());

    let additional_input_line = view! {
        <div style=move || {
            if input_mode.get() != InputMode::Batch { "" } else { "display: none;" }
        }>
            <form
                on:submit=move |ev| {
                    ev.prevent_default();
                    add_input()
                }
                style="display: flex; flex-direction: row;"
            >
                <Input
                    style:flex-grow="1"
                    style:min-width="0"
                    value=additional_input
                    disabled=disable_stop
                    placeholder=additional_input_string
                />
                <Button
                    disabled=disable_stop
                    class="green"
                    icon=icondata::AiSendOutlined
                    button_type=ButtonType::Submit
                />
            </form>
        </div>
    };

    let disable_input_editor =
        Memo::new(move |_| is_running.get() || input_mode.get() == InputMode::FullInteractive);

    let body = {
        let do_run = Box::new(do_run);
        let do_run2 = do_run.clone();
        view! {
            <StatusView state fetching_compiler_progress />
            <StorageErrorView />
            <div style="display: flex; flex-direction: column; height: calc(100vh - 65px);">
                <div style="flex-grow: 1; min-height: 0;">
                    <Grid cols=4 x_gap=8 class="textarea-grid">
                        <GridItem column=3>
                            <Editor
                                contents=code
                                cache_key="code"
                                syntax=Signal::derive(move || Some(lang.get()))
                                readonly=is_running
                                ctrl_enter=do_run
                                kb_mode=kb_mode
                                ls_interface=Some((
                                    ls_receiver,
                                    Box::new(move |s| send_worker_message(
                                        WorkerLSRequest::Message(s).into(),
                                    )),
                                ))
                            />
                        </GridItem>
                        <GridItem>
                            <div style="display: flex; flex-direction: column; height: 100%;">
                                {additional_input_line} <div style="flex: 1 1; min-height: 0;">
                                    <Editor
                                        contents=stdin
                                        cache_key="stdin"
                                        syntax=None
                                        readonly=disable_input_editor
                                        ctrl_enter=do_run2
                                        kb_mode=kb_mode
                                        ls_interface=None
                                    />
                                </div>
                            </div>
                        </GridItem>
                    </Grid>
                </div>
                <div>
                    <OutputView state />
                </div>
            </div>
        }
    };

    view! {
        <Layout position=LayoutPosition::Absolute content_style="width: 100%; height: 100%;">
            <LayoutHeader>{navbar}</LayoutHeader>
            {body}
        </Layout>
    }
}

fn main() {
    init_logging();

    let large_file_set = RwSignal::new(LargeFileSet::default());

    mount_to_body(move || {
        view! {
            <I18nContextProvider>
                <ConfigProvider>
                    <Provider value=large_file_set>
                        <App />
                    </Provider>
                </ConfigProvider>
            </I18nContextProvider>
        }
    })
}
