leptos_i18n::load_locales!();

use std::collections::HashMap;
use std::ops::Deref;
use std::{collections::HashSet, time::Duration};

use anyhow::Result;
use async_channel::{unbounded, Sender};
use common::{
    init_logging, File, Language, WorkerExecRequest, WorkerExecResponse, WorkerLSRequest,
    WorkerLSResponse, WorkerRequest, WorkerResponse,
};
use gloo_timers::future::sleep;
use leptos::{context::Provider, prelude::*};
use serde::{Deserialize, Serialize};
use thaw::{
    Button, ButtonType, ConfigProvider, Flex, FlexAlign, Grid, GridItem, Input, Layout,
    LayoutHeader, LayoutPosition, MessageBar, MessageBarActions, MessageBarBody, MessageBarIntent,
    MessageBarLayout, MessageBarTitle,
};
use tracing::{debug, info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{MessageEvent, MouseEvent, Worker, WorkerOptions, WorkerType};

use i18n::*;

mod editor;
mod enum_select;
mod output;
mod settings;
mod theme;
mod util;

use crate::editor::{Editor, EditorText};
use crate::enum_select::EnumSelect;
use crate::output::{OutputControl, OutputView};
use crate::settings::Settings;
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
    NotStarted,
    MessageSent,
    FetchingCompiler,
    CompilationInProgress(Outcome, bool),
    InProgress(Outcome, bool),
    Complete(Outcome),
    Error(String, Outcome),
}

impl RunState {
    fn can_start(&self) -> bool {
        match self {
            RunState::Loading
            | RunState::MessageSent
            | RunState::FetchingCompiler
            | RunState::InProgress(_, _)
            | RunState::CompilationInProgress(_, _) => false,
            RunState::Complete(_) | RunState::Error(_, _) | RunState::NotStarted => true,
        }
    }
    fn can_stop(&self) -> bool {
        match self {
            RunState::Loading
            | RunState::Complete(_)
            | RunState::Error(_, _)
            | RunState::InProgress(_, false)
            | RunState::CompilationInProgress(_, false)
            | RunState::NotStarted => false,
            RunState::MessageSent
            | RunState::FetchingCompiler
            | RunState::InProgress(_, true)
            | RunState::CompilationInProgress(_, true) => true,
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

#[component]
fn StatusView(
    state: RwSignal<RunState>,
    fetching_compiler_progress: RwSignal<FetchingCompilerProgress>,
) -> impl IntoView {
    let i18n = use_i18n();

    move || {
        match state.read().deref() {
            RunState::Complete(_) => ().into_any(),
            RunState::CompilationInProgress(_, true) => view! {
                <MessageBar class="status-view" intent=MessageBarIntent::Success>
                    <MessageBarBody>{t!(i18n, compiling)}</MessageBarBody>
                </MessageBar>
            }
            .into_any(),
            RunState::InProgress(_, true) => view! {
                <MessageBar class="status-view" intent=MessageBarIntent::Success>
                    <MessageBarBody>{t!(i18n, executing)}</MessageBarBody>
                </MessageBar>
            }
            .into_any(),
            RunState::InProgress(_, false) | RunState::CompilationInProgress(_, false) => view! {
                <MessageBar class="status-view" intent=MessageBarIntent::Warning>
                    <MessageBarBody>{t!(i18n, stopping_execution)}</MessageBarBody>
                </MessageBar>
            }
            .into_any(),
            RunState::Error(err, _) => {
                let err = err.clone();
                view! {
                    <MessageBar
                        class="status-view"
                        intent=MessageBarIntent::Error
                        layout=MessageBarLayout::Multiline
                    >
                        <MessageBarBody>
                            <MessageBarTitle>{t!(i18n, error)}</MessageBarTitle>
                            <pre>{err}</pre>
                        </MessageBarBody>
                        <MessageBarActions>
                            <Button
                                class="red"
                                icon=icondata::AiCloseOutlined
                                on_click=move |_| {
                                    state
                                        .update(|s| {
                                            if let RunState::Error(_, o) = s {
                                                *s = RunState::Complete(std::mem::take(o));
                                            }
                                        })
                                }
                                block=true
                            >
                                {t!(i18n, hide_error)}
                            </Button>
                        </MessageBarActions>
                    </MessageBar>
                }
                .into_any()
            }
            RunState::NotStarted => ().into_any(),
            RunState::Loading => view! {
                <MessageBar class="status-view" intent=MessageBarIntent::Success>
                    <MessageBarBody>{t!(i18n, loading)}</MessageBarBody>
                </MessageBar>
            }
            .into_any(),
            RunState::FetchingCompiler | RunState::MessageSent => view! {
                <MessageBar
                    class="status-view"
                    intent=MessageBarIntent::Success
                    layout=MessageBarLayout::Multiline
                >
                    <MessageBarBody>
                        <MessageBarTitle>{t!(i18n, downloading_runtime)}</MessageBarTitle>
                        <For
                            each=move || fetching_compiler_progress.get()
                            key=|x| x.clone()
                            let((name, progress))
                        >
                            <div style="display: flex; flex-direction: row; align-items: center; gap: 20px;">
                                <pre>{name}</pre>
                                <progress value=progress.map(|x| x.0) max=progress.map(|x| x.1) />
                                <span style="width: 3em; text-align: right;">
                                    {progress
                                        .map(|(cur, tot)| {
                                            format!("{:.1}%", 100. * cur as f64 / tot as f64)
                                        })}
                                </span>
                            </div>
                        </For>
                    </MessageBarBody>
                </MessageBar>
            }
            .into_any(),
        }
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
    let msg = match msg {
        WorkerResponse::Execution(msg) => msg,
        WorkerResponse::LS(msg) => {
            ls_message_chan.try_send(msg)?;
            return Ok(());
        }
        WorkerResponse::FetchingCompiler(name, progress) => {
            fetching_compiler_progress.update(|x| {
                x.insert(name, progress);
            });
            return Ok(());
        }
        WorkerResponse::CompilerFetchDone(name) => {
            fetching_compiler_progress.update(|x| {
                x.remove(&name);
            });
            return Ok(());
        }
    };
    // Avoid running state.update if it is not changing the actual state. This helps avoiding too
    // many slowdowns due to the reactive system recomputing state.
    if state.with_untracked(|s| {
        matches!(
            (&msg, s),
            (
                WorkerExecResponse::StdoutChunk(_)
                    | WorkerExecResponse::StderrChunk(_)
                    | WorkerExecResponse::CompilationMessageChunk(_),
                RunState::InProgress(_, false),
            )
        )
    }) {
        return Ok(());
    }

    state.update(|mut state| {
        match (msg, &mut state) {
            (WorkerExecResponse::Done, RunState::InProgress(cur, _)) => {
                *state = RunState::Complete(std::mem::take(cur));
            }
            (WorkerExecResponse::CompilationDone, RunState::CompilationInProgress(cur, _)) => {
                *state = RunState::InProgress(std::mem::take(cur), true);
            }
            (WorkerExecResponse::Error(s), RunState::MessageSent | RunState::FetchingCompiler) => {
                *state = RunState::Error(s, Outcome::default());
            }
            (
                WorkerExecResponse::Error(s),
                RunState::InProgress(cur, _) | RunState::CompilationInProgress(cur, _),
            ) => {
                *state = RunState::Error(s, std::mem::take(cur));
            }
            (
                WorkerExecResponse::StdoutChunk(chunk),
                RunState::InProgress(cur, true) | RunState::CompilationInProgress(cur, true),
            ) => {
                cur.stdout.extend_from_slice(&chunk);
            }
            (
                WorkerExecResponse::StderrChunk(chunk),
                RunState::InProgress(cur, true) | RunState::CompilationInProgress(cur, true),
            ) => {
                cur.stderr.extend_from_slice(&chunk);
            }
            (
                WorkerExecResponse::CompilationMessageChunk(chunk),
                RunState::InProgress(cur, true) | RunState::CompilationInProgress(cur, true),
            ) => {
                cur.compile_stderr.extend_from_slice(&chunk);
            }
            (WorkerExecResponse::Ready, RunState::Loading) => {
                *state = RunState::NotStarted;
            }
            (WorkerExecResponse::Started, RunState::MessageSent) => {
                *state = RunState::FetchingCompiler;
            }
            (WorkerExecResponse::CompilerFetched, RunState::FetchingCompiler) => {
                *state = RunState::CompilationInProgress(Outcome::default(), true);
            }
            (msg, _) => {
                warn!("unexpected msg & state combination: {msg:?} {state:?}");
            }
        };
    });

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

    let (sender, receiver) = unbounded();

    let fetching_compiler_progress = RwSignal::new(FetchingCompilerProgress::default());

    worker.set_onmessage(Some(
        Closure::<dyn Fn(_)>::new(move |msg| {
            handle_message(msg, state, fetching_compiler_progress, &sender).unwrap();
        })
        .into_js_value()
        .unchecked_ref(),
    ));

    let send_worker_message = {
        let (sender, receiver) = unbounded::<WorkerRequest>();
        spawn_local(async move {
            loop {
                if !matches!(state.get_untracked(), RunState::Loading) {
                    break;
                }
                sleep(Duration::from_millis(50)).await;
            }
            loop {
                let msg = receiver.recv().await.expect("frontend died?");
                debug!("send to worker: {:?}", msg);
                worker
                    .post_message(
                        &serde_wasm_bindgen::to_value(&msg).expect("invalid message to worker"),
                    )
                    .expect("worker died");
            }
        });

        move |m: WorkerRequest| {
            sender.try_send(m).expect("worker died?");
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
    let is_running = Memo::new(move |_| state.with(|s| s.can_stop() || !s.can_start()));

    let lang = RwSignal::new(load("language").unwrap_or(Language::CPP));
    let lang_options = [Language::CPP, Language::C, Language::Python]
        .into_iter()
        .map(|lng| (lng, Signal::stored(lng.into())))
        .collect::<Vec<_>>();

    {
        let send_worker_message = send_worker_message.clone();
        Effect::new(move |_| {
            let lang = lang.get();
            info!("Requesting language server for {lang:?}");
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
            state.set(RunState::MessageSent);
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

    let on_stop = {
        let send_worker_message = send_worker_message.clone();
        move |_: MouseEvent| {
            state.update(|x| {
                if let RunState::CompilationInProgress(_, accept)
                | RunState::InProgress(_, accept) = x
                {
                    *accept = false;
                } else {
                    warn!("asked to stop while not running");
                }
            });
            info!("Stopping execution");
            send_worker_message(WorkerExecRequest::Cancel.into());
        }
    };

    let show_stdout = RwSignal::new(true);
    let show_stderr = RwSignal::new(false);
    let show_compilation = RwSignal::new(true);

    Effect::new(move |_| {
        save("language", &lang.get());
        if lang.get() == Language::Python {
            if show_compilation.get_untracked() && !show_stderr.get_untracked() {
                show_stderr.set(true);
                show_compilation.set(false);
            }
        } else if !show_compilation.get_untracked() && show_stderr.get_untracked() {
            show_stderr.set(false);
            show_compilation.set(true);
        }
    });

    let kb_mode = RwSignal::new(load("kb_mode").unwrap_or(KeyboardMode::Standard));
    Effect::new(move |_| save("kb_mode", &kb_mode.get()));

    let show_output_tooltip = Signal::derive(move || t_display!(i18n, show_output).to_string());
    let show_stderr_tooltip = Signal::derive(move || t_display!(i18n, show_stderr).to_string());
    let show_compileerr_tooltip =
        Signal::derive(move || t_display!(i18n, show_compileerr).to_string());

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
                <Button
                    disabled=disable_stop
                    class="red"
                    icon=icondata::AiCloseOutlined
                    on_click=on_stop
                >
                    {t!(i18n, stop)}
                </Button>
                <Button
                    disabled=disable_start
                    class="green"
                    loading=is_running
                    icon=icondata::AiCaretRightFilled
                    on_click=move |_| do_run()
                >
                    {t!(i18n, run)}
                </Button>
                <OutputControl
                    signal=show_stdout
                    icon=icondata::VsOutput
                    tooltip=show_output_tooltip
                    color="blue"
                />
                <OutputControl
                    signal=show_stderr
                    icon=icondata::BiErrorSolid
                    tooltip=show_stderr_tooltip
                    color="yellow"
                />
                <OutputControl
                    signal=show_compilation
                    icon=icondata::BiCommentErrorSolid
                    tooltip=show_compileerr_tooltip
                    color="yellow"
                />
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

    let disable_input_editor = {
        Memo::new(move |_| disable_start.get() || input_mode.get() == InputMode::FullInteractive)
    };

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
                                readonly=disable_start
                                ctrl_enter=do_run.clone()
                                kb_mode=kb_mode
                                ls_interface=Some((
                                    receiver,
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
                    <OutputView state show_stdout show_stderr show_compilation />
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
