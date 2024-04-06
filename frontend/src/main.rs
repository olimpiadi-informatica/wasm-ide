use std::{borrow::Cow, collections::HashSet, time::Duration};

use async_channel::{unbounded, Sender};
use common::{ClientMessage, KeyboardMode, Language, WorkerMessage};
use gloo_timers::future::sleep;
use icondata::Icon;
use leptos::*;
use leptos_use::signal_throttled;
use thaw::{
    use_rw_theme, Alert, AlertVariant, Button, ButtonColor, ButtonVariant, Divider, GlobalStyle,
    Grid, GridItem, Icon, Layout, LayoutHeader, Popover, PopoverTrigger, Scrollbar, Select,
    SelectOption, Space, SpaceAlign, Text, Theme, ThemeProvider, Upload,
};
use wasm_bindgen::prelude::*;

use anyhow::Result;
use log::{info, warn};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    FileList, HtmlAnchorElement, MessageEvent, MouseEvent, Worker, WorkerOptions, WorkerType,
};

mod editor;

use editor::{Editor, EditorText, LSEvent};

struct LargeFileSet(HashSet<String>);

#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub stdout: Vec<u8>,
    pub compile_stderr: Vec<u8>,
    pub stderr: Vec<u8>,
}

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

trait Stringifiable: Sized {
    fn stringify(&self) -> Cow<'_, str>;
    fn from_string(data: String) -> Option<Self>;
}

impl Stringifiable for EditorText {
    fn stringify(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.text())
    }
    fn from_string(data: String) -> Option<EditorText> {
        Some(EditorText::from_text(data))
    }
}

impl<T: serde::Serialize + for<'a> serde::Deserialize<'a>> Stringifiable for T {
    fn stringify(&self) -> Cow<'_, str> {
        Cow::Owned(serde_json::to_string(self).expect("serialization error"))
    }
    fn from_string(data: String) -> Option<Self> {
        serde_json::from_str(&data).ok()
    }
}

fn save<T: Stringifiable>(key: &str, value: &T) {
    let s = value.stringify();
    let large_files = expect_context::<RwSignal<LargeFileSet>>();
    if s.len() >= 3_000_000 {
        large_files.update(|x| {
            x.0.insert(key.to_owned());
        });
        return;
    }
    large_files.update(|x| {
        x.0.remove(key);
    });
    window()
        .local_storage()
        .expect("no local storage")
        .unwrap()
        .set(key, &s)
        .expect("could not save data");
}

fn load<T: Stringifiable>(key: &str) -> Option<T> {
    window()
        .local_storage()
        .expect("no local storage")
        .unwrap()
        .get(key)
        .expect("error fetching from local storage")
        .map(|x| T::from_string(x))
        .flatten()
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
            | RunState::MessageSent
            | RunState::FetchingCompiler
            | RunState::Complete(_)
            | RunState::Error(_, _)
            | RunState::InProgress(_, false)
            | RunState::CompilationInProgress(_, false)
            | RunState::NotStarted => false,
            RunState::InProgress(_, true) | RunState::CompilationInProgress(_, true) => true,
        }
    }
    fn has_output(&self) -> bool {
        match self {
            RunState::Loading
            | RunState::MessageSent
            | RunState::FetchingCompiler
            | RunState::CompilationInProgress(_, _)
            | RunState::InProgress(_, _)
            | RunState::Error(_, _)
            | RunState::NotStarted => false,
            RunState::Complete(_) => true,
        }
    }
}

#[component]
pub fn SelectOption(is: &'static str, value: ReadSignal<String>) -> impl IntoView {
    view! {
        <option value=is selected=move || value.get() == is>
            {is}
        </option>
    }
}

#[component]
fn StorageErrorView() -> impl IntoView {
    let large_files = expect_context::<RwSignal<LargeFileSet>>();
    move || {
        large_files.with(|lf| {
            if lf.0.is_empty() {
                view! {}.into_view()
            } else {
                view! {
                    <div class="storage-error-view" >
                        <Alert variant=AlertVariant::Warning>"Alcuni file sono troppo grandi e non possono essere salvati."</Alert>
                    </div>
                }.into_view()
            }
        })
    }
}

#[component]
fn StatusView(state: RwSignal<RunState>) -> impl IntoView {
    let state2 = state.clone();
    let state_to_view = move |state: &RunState| {
        match state {
        RunState::Complete(_) => {
            view! { <Alert variant=AlertVariant::Success>"Esecuzione completata!"</Alert> }
                .into_view()
        }
        RunState::CompilationInProgress(_, true) => {
            view! { <Alert variant=AlertVariant::Success>"Compilazione in corso..."</Alert> }
                .into_view()
        }
        RunState::InProgress(_, true) => {
            view! { <Alert variant=AlertVariant::Success>"Esecuzione in corso..."</Alert> }
                .into_view()
        }
        RunState::InProgress(_, false) | RunState::CompilationInProgress(_, false) => {
            view! { <Alert variant=AlertVariant::Warning>"Interruzione dell'esecuzione in corso..."</Alert> }
                .into_view()
        }
        RunState::Error(err, _) => {
            let err = err.clone();
            if err.is_empty() {
                view! {
                    <Alert variant=AlertVariant::Error title="Error">
                        ""
                    </Alert>
                }
                .into_view()
            } else {
                view! {
                    <Alert variant=AlertVariant::Error title="Error">
                        <pre>{err}</pre>
                        <Button
                            color=ButtonColor::Error
                            icon=icondata::AiCloseOutlined
                            on_click=move |_| {
                                state2
                                    .update(|s| {
                                        if let RunState::Error(err, _) = s {
                                            *err = String::new();
                                        }
                                    })
                            }

                            block=true
                        >
                            "Nascondi errore"
                        </Button>
                    </Alert>
                }
                .into_view()
            }
        }
        RunState::NotStarted => {
            view! { <Alert variant=AlertVariant::Success>"Clicca \"Esegui\" per eseguire"</Alert> }
                .into_view()
        }
        RunState::Loading => {
            view! { <Alert variant=AlertVariant::Success>"Loading..."</Alert> }.into_view()
        }
        RunState::FetchingCompiler | RunState::MessageSent => {
            view! { <Alert variant=AlertVariant::Success>"Downloading runtime..."</Alert> }
                .into_view()
        }
    }
    };

    view! { <div class="status-view">{move || state.with(state_to_view)}</div> }
}

fn output_for_display(s: &[u8]) -> String {
    const LEN_LIMIT: usize = 16 * 1024;
    let (data, extra) = if s.len() < LEN_LIMIT {
        (s, "")
    } else {
        (&s[..LEN_LIMIT], "...")
    };
    format!("{}{}", String::from_utf8_lossy(data), extra)
}

#[component]
fn OutDiv(
    #[prop(into)] state: MaybeSignal<RunState>,
    #[prop(into)] display: MaybeSignal<bool>,
    get_data: fn(&Outcome) -> &Vec<u8>,
    icon: Icon,
) -> impl IntoView {
    move || {
        if !display.get() {
            view! {}.into_view()
        } else {
            state.with(move |s| {
                let (additional_style, txt) = match s {
                    RunState::InProgress(o, _) | RunState::Error(_, o) | RunState::Complete(o) => {
                        ("", output_for_display(get_data(o)))
                    }
                    _ => ("color: #888;", "programma non ancora eseguito".to_string()),
                };

                let pre_style = format!("width: 100%; text-align: left; {}", additional_style,);

                view! {
                    <div style="flex-grow: 1; flex-basis: 0; flex-shrink: 1; text-align: center;">
                        <Icon icon style="font-size: 1.5em"/>
                        <Divider class="outdivider"/>
                        <Scrollbar style="height: 18vh;">
                            <pre style=pre_style>{txt}</pre>
                        </Scrollbar>
                    </div>
                }
                .into_view()
            })
        }
    }
}

#[component]
fn OutputView(
    state: RwSignal<RunState>,
    #[prop(into)] show_stdout: MaybeSignal<bool>,
    #[prop(into)] show_stderr: MaybeSignal<bool>,
    #[prop(into)] show_compilation: MaybeSignal<bool>,
) -> impl IntoView {
    let state = signal_throttled(state, 100.0);
    move || {
        if !show_stdout.get() && !show_stderr.get() && !show_compilation.get() {
            view! {}.into_view()
        } else {
            view! {
                <div style="display: flex; flex-direction: row;">
                    <OutDiv
                        state
                        display=show_stdout
                        get_data=|outcome| &outcome.stdout
                        icon=icondata::VsOutput
                    />
                    <OutDiv
                        state
                        display=show_stderr
                        get_data=|outcome| &outcome.stderr
                        icon=icondata::BiErrorSolid
                    />
                    <OutDiv
                        state
                        display=show_compilation
                        get_data=|outcome| &outcome.compile_stderr
                        icon=icondata::BiCommentErrorSolid
                    />
                </div>
            }
            .into_view()
        }
    }
}

fn handle_message(
    msg: JsValue,
    state: RwSignal<RunState>,
    ls_message_chan: &Sender<LSEvent>,
) -> Result<()> {
    let msg = msg.dyn_into::<MessageEvent>().unwrap().data();
    let mut msg = match serde_wasm_bindgen::from_value::<WorkerMessage>(msg) {
        Ok(msg) => msg,
        Err(e) => {
            warn!("invalid message from worker: {e}");
            return Ok(());
        }
    };
    info!("{msg:?}");
    if let WorkerMessage::LSReady = msg {
        info!("LS ready");
        ls_message_chan.try_send(LSEvent::Ready)?;
        return Ok(());
    }
    if let WorkerMessage::LSStopping = msg {
        info!("LS ready");
        ls_message_chan.try_send(LSEvent::Stopping)?;
        return Ok(());
    }
    if let WorkerMessage::LSMessage(msg) = msg {
        info!("LS message received");
        ls_message_chan.try_send(LSEvent::Message(msg))?;
        return Ok(());
    }
    // Avoid running state.update if it is not changing the actual state. This helps avoiding too
    // many slowdowns due to the reactive system recomputing state.
    if state.with_untracked(|s| {
        matches!(
            (&msg, s),
            (
                WorkerMessage::StdoutChunk(_)
                    | WorkerMessage::StderrChunk(_)
                    | WorkerMessage::CompilationMessageChunk(_),
                RunState::InProgress(_, false),
            )
        )
    }) {
        return Ok(());
    }

    state.update(|mut state| {
        match (&mut msg, &mut state) {
            (WorkerMessage::Done, RunState::InProgress(cur, _)) => {
                *state = RunState::Complete(std::mem::take(cur));
            }
            (WorkerMessage::CompilationDone, RunState::CompilationInProgress(cur, _)) => {
                *state = RunState::InProgress(std::mem::take(cur), true);
            }
            (
                WorkerMessage::Error(s),
                RunState::InProgress(cur, _) | RunState::CompilationInProgress(cur, _),
            ) => {
                *state = RunState::Error(std::mem::take(s), std::mem::take(cur));
            }
            (
                WorkerMessage::StdoutChunk(chunk),
                RunState::InProgress(cur, true) | RunState::CompilationInProgress(cur, true),
            ) => {
                cur.stdout.extend_from_slice(&chunk);
            }
            (
                WorkerMessage::StderrChunk(chunk),
                RunState::InProgress(cur, true) | RunState::CompilationInProgress(cur, true),
            ) => {
                cur.stderr.extend_from_slice(&chunk);
            }
            (
                WorkerMessage::CompilationMessageChunk(chunk),
                RunState::InProgress(cur, true) | RunState::CompilationInProgress(cur, true),
            ) => {
                cur.compile_stderr.extend_from_slice(&chunk);
            }
            (WorkerMessage::Ready, RunState::Loading) => {
                *state = RunState::NotStarted;
            }
            (WorkerMessage::Started, RunState::MessageSent) => {
                *state = RunState::FetchingCompiler;
            }
            (WorkerMessage::CompilerFetched, RunState::FetchingCompiler) => {
                *state = RunState::CompilationInProgress(Outcome::default(), true);
            }
            _ => {
                warn!("unexpected msg & state combination: {msg:?} {state:?}");
            }
        };
    });

    Ok(())
}

#[component]
fn OutputControl(
    signal: RwSignal<bool>,
    icon: Icon,
    tooltip: &'static str,
    color: ButtonColor,
) -> impl IntoView {
    let variant = {
        let signal = signal.clone();
        Signal::derive(move || {
            if signal.get() {
                ButtonVariant::Primary
            } else {
                ButtonVariant::Outlined
            }
        })
    };
    view! {
        <Popover>
            <PopoverTrigger slot>
                <Button icon on_click=move |_| signal.update(|x| *x = !*x) color variant/>
            </PopoverTrigger>
            {tooltip}
        </Popover>
    }
}

const DEFAULT_CODE: &str = r#"#include <stdio.h>
#include <cmath>
int main() {
  long long n = 1000;
  long long variable_that_is_not_used = 1;
  printf("Hello world, computation started...\n");
  long long i = 0;
  for (size_t j = 0; j < n; j++) {
    if (std::sin(j) < 0.5) {
      i++;
    }
  }
  printf("Hello world %lld\n", i);
}"#;

const DEFAULT_STDIN: &str = "inserisci qui l'input...";

fn download(name: &str, data: &[u8]) {
    use base64::prelude::*;
    let b64 = BASE64_STANDARD.encode(data);
    let url = format!("data:text/plain;base64,{}", b64);
    let w = window();
    let d = w.document().expect("no document");
    let a = d
        .create_element("a")
        .unwrap()
        .dyn_into::<HtmlAnchorElement>()
        .unwrap();
    a.set_download(name);
    a.set_href(&url);
    d.body().expect("no body").append_child(&a).unwrap();
    a.click(); // TODO: this causes a panic for some reason
    a.remove();
}

#[component]
fn App() -> impl IntoView {
    let mut options = WorkerOptions::default();
    options.type_(WorkerType::Module);
    let worker =
        Worker::new_with_options("./start_worker.js", &options).expect("could not start worker");

    let theme = use_rw_theme();

    let theme_name_and_icon = create_memo(move |_| {
        theme.with(|theme: &Theme| {
            if theme.name == *"light" {
                ("Dark", icondata::BiMoonSolid)
            } else {
                ("Light", icondata::BiSunSolid)
            }
        })
    });
    let change_theme = move |_| {
        save("theme", &theme_name_and_icon.get_untracked().0.to_string());
        if theme_name_and_icon.get_untracked().0 == "Light" {
            theme.set(Theme::light());
        } else {
            theme.set(Theme::dark());
        }
    };

    let state = create_rw_signal(RunState::Loading);

    let (sender, receiver) = unbounded();

    worker.set_onmessage(Some(
        Closure::<dyn Fn(_)>::new(move |msg| {
            handle_message(msg, state, &sender).unwrap();
        })
        .into_js_value()
        .unchecked_ref(),
    ));

    let send_worker_message = {
        let (sender, receiver) = unbounded::<ClientMessage>();
        let state = state.clone();
        spawn_local(async move {
            loop {
                if !matches!(state.get_untracked(), RunState::Loading) {
                    break;
                }
                sleep(Duration::from_millis(50)).await;
            }
            loop {
                let msg = receiver.recv().await.expect("frontend died?");
                info!("send to worker: {:?}", msg);
                worker
                    .post_message(
                        &serde_wasm_bindgen::to_value(&msg).expect("invalid message to worker"),
                    )
                    .expect("worker died");
            }
        });

        move |m: ClientMessage| {
            sender.try_send(m).expect("worker died?");
        }
    };

    let code = create_rw_signal(load("code").unwrap_or_else(|| EditorText::from_str(DEFAULT_CODE)));
    let stdin =
        create_rw_signal(load("stdin").unwrap_or_else(|| EditorText::from_str(DEFAULT_STDIN)));

    let disable_start = {
        let state = state.clone();
        create_memo(move |_| state.with(|s| !s.can_start()))
    };
    let disable_stop = {
        let state = state.clone();
        create_memo(move |_| state.with(|s| !s.can_stop()))
    };
    let is_running = {
        let state = state.clone();
        create_memo(move |_| state.with(|s| s.can_stop() || !s.can_start()))
    };
    let disable_output = {
        let state = state.clone();
        create_memo(move |_| state.with(|s| !s.has_output()))
    };

    let upload_input = move |files: FileList| {
        let file = files.get(0).expect("0 files?");
        spawn_local(async move {
            let promise = file.text();
            let text = JsFuture::from(promise).await;
            match text {
                Ok(text) => {
                    let text =
                        EditorText::from_text(text.as_string().expect("did not read a string"));
                    save("stdin", &text);
                    stdin.set(text)
                }
                Err(err) => warn!("could not read file: {err:?}"),
            }
        });
    };

    let download_output = move |_| {
        let data = state.with(|s| {
            let RunState::Complete(outcome) = s else {
                warn!("requested download in invalid state");
                return None;
            };
            Some(outcome.stdout.clone())
        });
        let Some(data) = data else {
            return;
        };
        download("output.txt", &data);
    };

    let lang = load("language").unwrap_or(Language::CPP);
    let lang = create_rw_signal(Some(lang));

    let download_code = move |_| {
        let code = code.with_untracked(|x| x.text().clone());
        match lang.get_untracked().unwrap_or(Language::CPP) {
            Language::C => download("code.c", code.as_bytes()),
            Language::CPP => download("code.cpp", code.as_bytes()),
            Language::Python => download("code.py", code.as_bytes()),
        }
    };

    {
        let lang = lang.clone();
        let send_worker_message = send_worker_message.clone();
        create_effect(move |_| {
            let lang = lang.get().unwrap();
            let window = web_sys::window().expect("no window available");
            let base_url = window.location().href().expect("could not get href");
            send_worker_message(ClientMessage::StartLS(base_url, lang));
        });
    }

    let languages: Vec<_> = [Language::CPP, Language::C, Language::Python]
        .into_iter()
        .map(|x| SelectOption {
            value: x,
            label: x.into(),
        })
        .collect();

    let do_run = {
        let send_worker_message = send_worker_message.clone();
        move || {
            state.set(RunState::MessageSent);
            let send_worker_message = send_worker_message.clone();
            spawn_local(async move {
                code.with_untracked(|x| x.await_all_changes()).await;
                stdin.with_untracked(|x| x.await_all_changes()).await;
                let code = code.with_untracked(|x| x.text().clone());
                let input = stdin.with_untracked(|x| x.text().clone());
                let window = web_sys::window().expect("no window available");
                let base_url = window.location().href().expect("could not get href");
                send_worker_message(ClientMessage::Compile {
                    base_url,
                    source: code,
                    language: lang.get_untracked().unwrap_or(Language::CPP),
                    input: input.into_bytes(),
                });
            });
        }
    };

    let on_stop = {
        let send_worker_message = send_worker_message.clone();
        move |_: MouseEvent| {
            state.update(|x| {
                if let RunState::InProgress(_, accept) = x {
                    *accept = false;
                } else {
                    warn!("asked to stop while not running");
                }
            });
            send_worker_message(ClientMessage::Cancel);
        }
    };

    let show_stdout = create_rw_signal(true);
    let show_stderr = create_rw_signal(false);
    let show_compilation = create_rw_signal(true);

    create_effect(move |_| {
        save("language", &lang.get().unwrap_or(Language::CPP));
        if lang.get().is_some_and(|x| x == Language::Python) {
            if show_compilation.get_untracked() && !show_stderr.get_untracked() {
                show_stderr.set(true);
                show_compilation.set(false);
            }
        } else {
            if !show_compilation.get_untracked() && show_stderr.get_untracked() {
                show_stderr.set(false);
                show_compilation.set(true);
            }
        }
    });

    let kb_mode = load("kb_mode").unwrap_or(KeyboardMode::Standard);
    let kb_mode = create_rw_signal(Some(kb_mode));
    let kb_modes: Vec<_> = [
        KeyboardMode::Standard,
        KeyboardMode::Vim,
        KeyboardMode::Emacs,
    ]
    .into_iter()
    .map(|x| SelectOption {
        value: x,
        label: x.into(),
    })
    .collect();

    create_effect(move |_| save("kb_mode", &kb_mode.get().unwrap_or(KeyboardMode::Standard)));

    let navbar = {
        let do_run = do_run.clone();
        view! {
            <Space align=SpaceAlign::Center>
                <Button variant=ButtonVariant::Text on_click=change_theme>
                    {move || {
                        let (name, icon) = theme_name_and_icon.get();
                        view! {
                            <Icon icon style="padding: 0 5px 0 0;" width="1.5em" height="1.5em"/>
                            <Text>{name}</Text>
                        }
                    }}

                </Button>
                <Select value=lang options=languages class="language-selector"/>
                <Upload custom_request=upload_input>
                    <Button disabled=disable_start icon=icondata::AiUploadOutlined>
                        "Carica input"
                    </Button>
                </Upload>
                <Button
                    disabled=disable_stop
                    color=ButtonColor::Error
                    variant=ButtonVariant::Primary
                    icon=icondata::AiCloseOutlined
                    on_click=on_stop
                >
                    "Stop"
                </Button>
                <Button
                    disabled=disable_start
                    color=ButtonColor::Success
                    variant=ButtonVariant::Primary
                    loading=is_running
                    icon=icondata::AiCaretRightFilled
                    on_click=move |_| do_run()
                >
                    "Esegui"
                </Button>
                <Button
                    disabled=disable_output
                    color=ButtonColor::Success
                    variant=ButtonVariant::Primary
                    icon=icondata::AiDownloadOutlined
                    on_click=download_output
                >
                    "Scarica output"
                </Button>
                <Button
                    color=ButtonColor::Success
                    variant=ButtonVariant::Primary
                    icon=icondata::AiDownloadOutlined
                    on_click=download_code
                >
                    "Scarica codice"
                </Button>
                <OutputControl
                    signal=show_stdout
                    icon=icondata::VsOutput
                    tooltip="Mostra output"
                    color=ButtonColor::Primary
                />
                <OutputControl
                    signal=show_stderr
                    icon=icondata::BiErrorSolid
                    tooltip="Mostra errori runtime"
                    color=ButtonColor::Warning
                />
                <OutputControl
                    signal=show_compilation
                    icon=icondata::BiCommentErrorSolid
                    tooltip="Mostra messaggi di compilazione"
                    color=ButtonColor::Warning
                />
                <Select value=kb_mode options=kb_modes class="kb-selector"/>
            </Space>
        }
    };

    let body = {
        let do_run = Box::new(do_run);
        let do_run2 = do_run.clone();
        view! {
            <StatusView state/>
            <StorageErrorView />
            <div style="display: flex; flex-direction: column; height: calc(100vh - 65px);">
                <div style="flex-grow: 1;">
                    <Grid cols=4 x_gap=8 class="textarea-grid">
                        <GridItem column=3>
                            <Editor
                                contents=code
                                cache_key="code"
                                syntax=lang
                                readonly=disable_start
                                ctrl_enter=do_run.clone()
                                kb_mode=kb_mode
                                ls_interface=Some((receiver, Box::new(move |s| send_worker_message(ClientMessage::LSMessage(s)))))
                            />
                        </GridItem>
                        <GridItem>
                            <Editor
                                contents=stdin
                                cache_key="stdin"
                                syntax=None
                                readonly=disable_start
                                ctrl_enter=do_run2
                                kb_mode=kb_mode
                                ls_interface=None
                            />
                        </GridItem>
                    </Grid>
                </div>
                <div>
                    <OutputView state show_stdout show_stderr show_compilation/>
                </div>
            </div>
        }
    };

    view! {
        <Layout style="height: 100%;" content_style="height: 100%;">
            <LayoutHeader style="padding: 0 20px; display: flex; align-items: center; height: 64px; justify-content: space-between;">
                {navbar}
            </LayoutHeader>
            <Layout>{body}</Layout>
        </Layout>
    }
}

fn main() {
    console_error_panic_hook::set_once();
    console_log::init_with_level(log::Level::Debug).unwrap();

    let theme = if load("theme") == Some("Light".to_owned()) {
        Theme::light()
    } else {
        Theme::dark()
    };

    let large_files = create_rw_signal(LargeFileSet(HashSet::new()));
    provide_context(large_files);

    let theme = create_rw_signal(theme);

    mount_to_body(move || {
        view! {
            <ThemeProvider theme>
                <GlobalStyle/>
                <App/>
            </ThemeProvider>
        }
    })
}
