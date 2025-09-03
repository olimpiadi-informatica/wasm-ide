#![allow(clippy::type_complexity)]

leptos_i18n::load_locales!();

use std::{borrow::Cow, collections::HashSet, str::Chars, time::Duration};

use async_channel::{unbounded, Sender};
use common::{init_logging, Language, WorkerRequest, WorkerResponse};
use gloo_timers::future::sleep;
use icondata::Icon;
use leptos::*;
use leptos_use::signal_throttled;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thaw::{
    create_component_ref, use_rw_theme, Alert, AlertVariant, Button, ButtonColor, ButtonVariant,
    ComponentRef, Divider, GlobalStyle, Grid, GridItem, Icon, Input, Layout, LayoutHeader, Popover,
    PopoverTrigger, Scrollbar, ScrollbarRef, Select, SelectOption, Space, SpaceAlign, Text, Theme,
    ThemeProvider, Upload,
};
use wasm_bindgen::prelude::*;

use anyhow::{bail, ensure, Result};
use tracing::{debug, info, warn};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    FileList, HtmlAnchorElement, MessageEvent, MouseEvent, ScrollToOptions, Worker, WorkerOptions,
    WorkerType,
};

use i18n::*;

mod editor;

use editor::{Editor, EditorText, LSEvent};

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

impl<T: Serialize + DeserializeOwned> Stringifiable for T {
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
        .and_then(|x| T::from_string(x))
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
    let i18n = use_i18n();
    let large_files = expect_context::<RwSignal<LargeFileSet>>();
    move || {
        large_files.with(|lf| {
            if lf.0.is_empty() {
                view! {}.into_view()
            } else {
                view! {
                    <div class="storage-error-view">
                        <Alert variant=AlertVariant::Warning>{t!(i18n, files_too_big)}</Alert>
                    </div>
                }
                .into_view()
            }
        })
    }
}

#[component]
fn StatusView(state: RwSignal<RunState>) -> impl IntoView {
    let i18n = use_i18n();
    let state2 = state;
    let state_to_view = move |state: &RunState| match state {
        RunState::Complete(_) => {
            view! { <Alert variant=AlertVariant::Success>{t!(i18n, execution_completed)}</Alert> }
                .into_view()
        }
        RunState::CompilationInProgress(_, true) => {
            view! { <Alert variant=AlertVariant::Success>{t!(i18n, compiling)}</Alert> }.into_view()
        }
        RunState::InProgress(_, true) => {
            view! { <Alert variant=AlertVariant::Success>{t!(i18n, executing)}</Alert> }.into_view()
        }
        RunState::InProgress(_, false) | RunState::CompilationInProgress(_, false) => {
            view! { <Alert variant=AlertVariant::Warning>{t!(i18n, stopping_execution)}</Alert> }
                .into_view()
        }
        RunState::Error(err, _) => {
            let err = err.clone();
            if err.is_empty() {
                view! {
                    <Alert variant=AlertVariant::Error title=t_display!(i18n, error).to_string()>
                        ""
                    </Alert>
                }
                .into_view()
            } else {
                view! {
                    <Alert variant=AlertVariant::Error title=t_display!(i18n, error).to_string()>
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
                            {t!(i18n, hide_error)}
                        </Button>
                    </Alert>
                }
                .into_view()
            }
        }
        RunState::NotStarted => {
            view! { <Alert variant=AlertVariant::Success>{t!(i18n, click_to_run)}</Alert> }
                .into_view()
        }
        RunState::Loading => {
            view! { <Alert variant=AlertVariant::Success>{t!(i18n, loading)}</Alert> }.into_view()
        }
        RunState::FetchingCompiler | RunState::MessageSent => {
            view! { <Alert variant=AlertVariant::Success>{t!(i18n, downloading_runtime)}</Alert> }
                .into_view()
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Style {
    bold: bool,
    fg: Option<&'static str>,
}

impl Style {
    fn style_str(&self) -> String {
        let mut parts = Vec::new();
        if self.bold {
            parts.push("font-weight: bold".to_string());
        }
        if let Some(fg) = self.fg {
            parts.push(format!("color: {fg}"));
        }
        parts.join("; ")
    }
}

fn ansi(text: &str) -> Vec<(Style, String)> {
    fn parse(style: &mut Style, iter: &mut Chars) -> Result<()> {
        ensure!(
            iter.next() == Some('['),
            "expected '[' at start of ANSI sequence"
        );
        let mut num = 0;
        for c in iter {
            if c.is_ascii_digit() {
                num = num * 10 + (c as u8 - b'0') as usize;
            } else if c == 'm' || c == ';' {
                match num {
                    0 => *style = Style::default(),
                    1 => style.bold = true,
                    30 => style.fg = Some("black"),
                    31 => style.fg = Some("red"),
                    32 => style.fg = Some("green"),
                    33 => style.fg = Some("yellow"),
                    34 => style.fg = Some("blue"),
                    35 => style.fg = Some("magenta"),
                    36 => style.fg = Some("cyan"),
                    37 => style.fg = Some("white"),
                    _ => bail!("unsupported ANSI code: {num}"),
                }
                num = 0;
                if c == 'm' {
                    break;
                }
            } else {
                bail!("unexpected character '{c}' in ANSI escape sequence");
            }
        }
        Ok(())
    }

    let mut style = Style::default();
    let mut iter = text.chars();
    let mut fragments: Vec<(Style, String)> = Vec::new();

    while let Some(c) = iter.next() {
        if c == '\x1b' {
            let style_backup = style;
            let iter_backup = iter.clone();
            match parse(&mut style, &mut iter) {
                Ok(()) => continue,
                Err(e) => {
                    warn!("error parsing ANSI escape sequence: {e}");
                    style = style_backup;
                    iter = iter_backup;
                }
            }
        }
        if let Some(last) = fragments.last_mut() {
            if last.0 == style {
                last.1.push(c);
                continue;
            }
        }
        fragments.push((style, c.to_string()));
    }

    fragments
}

#[component]
fn OutDivInner(
    #[prop(into)] state: Signal<RunState>,
    get_data: fn(&Outcome) -> &Vec<u8>,
    icon: Icon,
) -> impl IntoView {
    let i18n = use_i18n();
    let scrollbar: ComponentRef<ScrollbarRef> = create_component_ref();

    let style_and_text = Signal::derive(move || {
        state.with(move |s| match s {
            RunState::InProgress(o, _) | RunState::Error(_, o) | RunState::Complete(o) => {
                ("", output_for_display(get_data(o)))
            }
            _ => (
                "color: #888;",
                t_display!(i18n, not_yet_executed).to_string(),
            ),
        })
    });

    let style = Signal::derive(move || {
        format!("width: 100%; text-align: left; {}", style_and_text.get().0)
    });

    let text = Signal::derive(move || style_and_text.get().1);
    let fragments = Signal::derive(move || ansi(&text.get()));

    create_effect(move |_| {
        text.get();
        let scroll_options = ScrollToOptions::new();
        scroll_options.set_behavior(web_sys::ScrollBehavior::Smooth);
        if let Some(scrollbar) = scrollbar.get_untracked() {
            let height = scrollbar
                .content_ref
                .get_untracked()
                .map(|el| el.scroll_height())
                .unwrap_or(1 << 16);
            scroll_options.set_top(height as f64);
            scrollbar.scroll_to_with_scroll_to_options(&scroll_options);
        }
    });

    view! {
        <div style="flex-grow: 1; flex-basis: 0; flex-shrink: 1; text-align: center;">
            <Icon icon style="font-size: 1.5em"/>
            <Divider class="outdivider"/>
            <Scrollbar style="height: 18vh;" comp_ref=scrollbar>
                <pre style=style>{
                    move || fragments.with(|f| f.iter().map(|(style, text)| {
                        view! { <span style=style.style_str()>{text}</span> }.into_view()
                    }).collect::<Vec<_>>())
                }</pre>
            </Scrollbar>
        </div>
    }
}

#[component]
fn OutDiv(
    #[prop(into)] state: Signal<RunState>,
    #[prop(into)] display: MaybeSignal<bool>,
    get_data: fn(&Outcome) -> &Vec<u8>,
    icon: Icon,
) -> impl IntoView {
    view! {
        <Show when=move || display.get()>
            <OutDivInner state=state get_data=get_data icon=icon />
        </Show>
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
    let mut msg = match serde_wasm_bindgen::from_value::<WorkerResponse>(msg) {
        Ok(msg) => msg,
        Err(e) => {
            warn!("invalid message from worker: {e}");
            return Ok(());
        }
    };
    debug!("{msg:?}");
    if let WorkerResponse::LSReady = msg {
        ls_message_chan.try_send(LSEvent::Ready)?;
        return Ok(());
    }
    if let WorkerResponse::LSStopping = msg {
        ls_message_chan.try_send(LSEvent::Stopping)?;
        return Ok(());
    }
    if let WorkerResponse::LSMessage(msg) = msg {
        ls_message_chan.try_send(LSEvent::Message(msg))?;
        return Ok(());
    }
    // Avoid running state.update if it is not changing the actual state. This helps avoiding too
    // many slowdowns due to the reactive system recomputing state.
    if state.with_untracked(|s| {
        matches!(
            (&msg, s),
            (
                WorkerResponse::StdoutChunk(_)
                    | WorkerResponse::StderrChunk(_)
                    | WorkerResponse::CompilationMessageChunk(_),
                RunState::InProgress(_, false),
            )
        )
    }) {
        return Ok(());
    }

    state.update(|mut state| {
        match (&mut msg, &mut state) {
            (WorkerResponse::Done, RunState::InProgress(cur, _)) => {
                *state = RunState::Complete(std::mem::take(cur));
            }
            (WorkerResponse::CompilationDone, RunState::CompilationInProgress(cur, _)) => {
                *state = RunState::InProgress(std::mem::take(cur), true);
            }
            (WorkerResponse::Error(s), RunState::FetchingCompiler) => {
                *state = RunState::Error(std::mem::take(s), Outcome::default());
            }
            (
                WorkerResponse::Error(s),
                RunState::InProgress(cur, _) | RunState::CompilationInProgress(cur, _),
            ) => {
                *state = RunState::Error(std::mem::take(s), std::mem::take(cur));
            }
            (
                WorkerResponse::StdoutChunk(chunk),
                RunState::InProgress(cur, true) | RunState::CompilationInProgress(cur, true),
            ) => {
                cur.stdout.extend_from_slice(chunk);
            }
            (
                WorkerResponse::StderrChunk(chunk),
                RunState::InProgress(cur, true) | RunState::CompilationInProgress(cur, true),
            ) => {
                cur.stderr.extend_from_slice(chunk);
            }
            (
                WorkerResponse::CompilationMessageChunk(chunk),
                RunState::InProgress(cur, true) | RunState::CompilationInProgress(cur, true),
            ) => {
                cur.compile_stderr.extend_from_slice(chunk);
            }
            (WorkerResponse::Ready, RunState::Loading) => {
                *state = RunState::NotStarted;
            }
            (WorkerResponse::Started, RunState::MessageSent) => {
                *state = RunState::FetchingCompiler;
            }
            (WorkerResponse::CompilerFetched, RunState::FetchingCompiler) => {
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
    tooltip: Signal<String>,
    color: ButtonColor,
) -> impl IntoView {
    let variant = {
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

fn download(name: &str, data: &[u8]) {
    use base64::prelude::*;
    let b64 = BASE64_STANDARD.encode(data);
    let url = format!("data:text/plain;base64,{b64}");
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

fn locale_name(locale: Locale) -> &'static str {
    match locale {
        Locale::en => "English",
        Locale::it => "Italiano",
        Locale::es => "Español",
        Locale::ca => "Català",
    }
}

fn kb_mode_string(locale: Locale, kb_mode: KeyboardMode) -> String {
    match kb_mode {
        KeyboardMode::Vim => td_display!(locale, vim_mode),
        KeyboardMode::Emacs => td_display!(locale, emacs_mode),
        KeyboardMode::Standard => td_display!(locale, standard_mode),
    }
    .into()
}

fn input_mode_string(locale: Locale, input_mode: InputMode) -> String {
    match input_mode {
        InputMode::Batch => td_display!(locale, batch_input),
        InputMode::MixedInteractive => td_display!(locale, mixed_interactive_input),
        InputMode::FullInteractive => td_display!(locale, full_interactive_input),
    }
    .into()
}

#[component]
fn ThemeSelector() -> impl IntoView {
    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    enum ThemePlus {
        System,
        Light,
        Dark,
    }

    let preferred_dark = leptos_use::use_preferred_dark();
    let theme_plus = create_rw_signal(load("theme").unwrap_or(ThemePlus::System));
    let theme = use_rw_theme();

    create_effect(move |_| {
        let new_theme = match theme_plus.get() {
            ThemePlus::System => match preferred_dark.get() {
                true => Theme::dark(),
                false => Theme::light(),
            },
            ThemePlus::Light => Theme::light(),
            ThemePlus::Dark => Theme::dark(),
        };
        if new_theme.name != theme.get_untracked().name {
            theme.set(new_theme);
        }
    });

    let theme_name_and_icon = create_memo(move |_| match theme_plus.get() {
        ThemePlus::System => match preferred_dark.get() {
            true => ("System", icondata::BiMoonSolid),
            false => ("System", icondata::BiSunSolid),
        },
        ThemePlus::Light => ("Light", icondata::BiSunSolid),
        ThemePlus::Dark => ("Dark", icondata::BiMoonSolid),
    });
    let change_theme = move |_| {
        let new_theme = match theme_plus.get_untracked() {
            ThemePlus::System => ThemePlus::Light,
            ThemePlus::Light => ThemePlus::Dark,
            ThemePlus::Dark => ThemePlus::System,
        };
        save("theme", &new_theme);
        theme_plus.set(new_theme);
    };

    view! {
        <Button variant=ButtonVariant::Text on_click=change_theme>
            {move || {
                let (name, icon) = theme_name_and_icon.get();
                view! {
                    <Icon icon style="padding: 0 5px 0 0;" width="1.5em" height="1.5em"/>
                    <Text>{name}</Text>
                }
            }}
        </Button>
    }
}

#[component]
fn App() -> impl IntoView {
    let options = WorkerOptions::default();
    options.set_type(WorkerType::Module);
    let worker =
        Worker::new_with_options("./worker_loader.js", &options).expect("could not start worker");

    let i18n = use_i18n();
    let locales: Vec<_> = Locale::get_all()
        .iter()
        .cloned()
        .map(|x| SelectOption {
            value: x,
            label: locale_name(x).to_string(),
        })
        .collect();

    let current_locale = create_rw_signal(Some(load("locale").unwrap_or_else(|| {
        let window = web_sys::window().expect("Missing Window");
        let navigator = window.navigator();
        let preferences: Vec<_> = navigator
            .languages()
            .into_iter()
            .map(|x| x.as_string().unwrap())
            .collect();
        Locale::find_locale(&preferences)
    })));

    create_effect(move |_| {
        let locale = current_locale.get().unwrap();
        save("locale", &locale);
        i18n.set_locale(locale);
    });

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
        create_rw_signal(load("code").unwrap_or_else(|| EditorText::from_str(starting_code)));

    let starting_stdin = include_str!("../default_stdin.txt");

    let stdin =
        create_rw_signal(load("stdin").unwrap_or_else(|| EditorText::from_str(starting_stdin)));

    let disable_start = create_memo(move |_| state.with(|s| !s.can_start()));
    let disable_stop = create_memo(move |_| state.with(|s| !s.can_stop()));
    let is_running = create_memo(move |_| state.with(|s| s.can_stop() || !s.can_start()));
    let disable_output = create_memo(move |_| state.with(|s| !s.has_output()));

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
        let send_worker_message = send_worker_message.clone();
        create_effect(move |_| {
            let lang = lang.get().unwrap();
            info!("Requesting language server for {lang:?}");
            send_worker_message(WorkerRequest::StartLS(lang));
        });
    }

    let languages: Vec<_> = [Language::CPP, Language::C, Language::Python]
        .into_iter()
        .map(|x| SelectOption {
            value: x,
            label: x.into(),
        })
        .collect();

    let input_mode = load("input_mode").unwrap_or(InputMode::Batch);
    let input_mode = create_rw_signal(Some(input_mode));

    let do_run = {
        let send_worker_message = send_worker_message.clone();
        move || {
            state.set(RunState::MessageSent);
            let send_worker_message = send_worker_message.clone();
            spawn_local(async move {
                if input_mode.get_untracked().unwrap() == InputMode::FullInteractive {
                    stdin.set(EditorText::from_str(""));
                }
                code.with_untracked(|x| x.await_all_changes()).await;
                stdin.with_untracked(|x| x.await_all_changes()).await;
                let code = code.with_untracked(|x| x.text().clone());
                let input = stdin.with_untracked(|x| x.text().clone());
                let (input, addn_msg) = match input_mode.get_untracked().unwrap() {
                    InputMode::MixedInteractive => {
                        (None, Some(WorkerRequest::StdinChunk(input.into_bytes())))
                    }
                    InputMode::FullInteractive => (None, None),
                    InputMode::Batch => (Some(input.into_bytes()), None),
                };

                info!("Requesting execution");
                send_worker_message(WorkerRequest::CompileAndRun {
                    source: code,
                    language: lang.get_untracked().unwrap_or(Language::CPP),
                    input,
                });
                if let Some(addn_msg) = addn_msg {
                    send_worker_message(addn_msg);
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
            send_worker_message(WorkerRequest::Cancel);
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
        } else if !show_compilation.get_untracked() && show_stderr.get_untracked() {
            show_stderr.set(false);
            show_compilation.set(true);
        }
    });

    let kb_mode = load("kb_mode").unwrap_or(KeyboardMode::Standard);
    let kb_mode = create_rw_signal(Some(kb_mode));
    let kb_modes = Signal::derive(move || -> Vec<SelectOption<KeyboardMode>> {
        [
            KeyboardMode::Standard,
            KeyboardMode::Vim,
            KeyboardMode::Emacs,
        ]
        .into_iter()
        .map(|x| SelectOption {
            value: x,
            label: kb_mode_string(i18n.get_locale(), x),
        })
        .collect()
    });

    create_effect(move |_| save("kb_mode", &kb_mode.get().unwrap_or(KeyboardMode::Standard)));

    let input_modes = Signal::derive(move || -> Vec<SelectOption<InputMode>> {
        [
            InputMode::Batch,
            InputMode::MixedInteractive,
            InputMode::FullInteractive,
        ]
        .into_iter()
        .map(|x| SelectOption {
            value: x,
            label: input_mode_string(i18n.get_locale(), x),
        })
        .collect()
    });

    create_effect(move |_| save("input_mode", &input_mode.get().unwrap_or(InputMode::Batch)));

    let show_output_tooltip = Signal::derive(move || t_display!(i18n, show_output).to_string());
    let show_stderr_tooltip = Signal::derive(move || t_display!(i18n, show_stderr).to_string());
    let show_compileerr_tooltip =
        Signal::derive(move || t_display!(i18n, show_compileerr).to_string());

    let navbar = {
        let do_run = do_run.clone();
        view! {
            <Space align=SpaceAlign::Center>
                <ThemeSelector />
                <Select value=current_locale options=locales class="locale-selector"/>
                <Select value=lang options=languages class="language-selector"/>
                <Upload custom_request=upload_input>
                    <Button disabled=disable_start icon=icondata::AiUploadOutlined>
                        {t!(i18n, load_input)}
                    </Button>
                </Upload>
                <Button
                    disabled=disable_stop
                    color=ButtonColor::Error
                    variant=ButtonVariant::Primary
                    icon=icondata::AiCloseOutlined
                    on_click=on_stop
                >
                    {t!(i18n, stop)}
                </Button>
                <Button
                    disabled=disable_start
                    color=ButtonColor::Success
                    variant=ButtonVariant::Primary
                    loading=is_running
                    icon=icondata::AiCaretRightFilled
                    on_click=move |_| do_run()
                >
                    {t!(i18n, run)}
                </Button>
                <Button
                    disabled=disable_output
                    color=ButtonColor::Success
                    variant=ButtonVariant::Primary
                    icon=icondata::AiDownloadOutlined
                    on_click=download_output
                >
                    {t!(i18n, download_output)}
                </Button>
                <Button
                    color=ButtonColor::Success
                    variant=ButtonVariant::Primary
                    icon=icondata::AiDownloadOutlined
                    on_click=download_code
                >
                    {t!(i18n, download_code)}
                </Button>
                <OutputControl
                    signal=show_stdout
                    icon=icondata::VsOutput
                    tooltip=show_output_tooltip
                    color=ButtonColor::Primary
                />
                <OutputControl
                    signal=show_stderr
                    icon=icondata::BiErrorSolid
                    tooltip=show_stderr_tooltip
                    color=ButtonColor::Warning
                />
                <OutputControl
                    signal=show_compilation
                    icon=icondata::BiCommentErrorSolid
                    tooltip=show_compileerr_tooltip
                    color=ButtonColor::Warning
                />
                <Select value=kb_mode options=kb_modes class="kb-selector"/>
                <Select value=input_mode options=input_modes class="input-selector"/>
            </Space>
        }
    };

    let additional_input = create_rw_signal(String::from(""));

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
            send_worker_message(WorkerRequest::StdinChunk(extra.into_bytes()));
        }
    };

    let additional_input_string =
        Signal::derive(move || t_display!(i18n, additional_input).to_string());

    let additional_input_line = {
        let add_input2 = add_input.clone();
        view! {
            <div
                class="additional-input"
                style=move || {
                    if input_mode.get().unwrap() != InputMode::Batch
                    {
                        ""
                    } else {
                        "display: none;"
                    }
                }
            >

                <div style="display: flex; flex-direction: row; height: 100%;">
                    <form
                        on:submit=move |ev| {
                            ev.prevent_default();
                            add_input()
                        }

                        style="width: 100%;"
                    >
                        <Input
                            value=additional_input
                            disabled=disable_stop
                            placeholder=additional_input_string
                        />
                    </form>
                    <Button
                        disabled=disable_stop
                        color=ButtonColor::Success
                        variant=ButtonVariant::Primary
                        icon=icondata::AiSendOutlined
                        on_click=move |_| add_input2()
                    />
                </div>
            </div>
        }
    };

    let disable_input_editor = {
        create_memo(move |_| {
            disable_start.get() || input_mode.get() == Some(InputMode::FullInteractive)
        })
    };

    let body = {
        let do_run = Box::new(do_run);
        let do_run2 = do_run.clone();
        view! {
            <StatusView state/>
            <StorageErrorView/>
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
                                ls_interface=Some((
                                    receiver,
                                    Box::new(move |s| send_worker_message(
                                        WorkerRequest::LSMessage(s),
                                    )),
                                ))
                            />

                        </GridItem>
                        <GridItem>
                            <div style="display: flex; flex-direction: column; height: calc(75vh);">
                                {additional_input_line} <div style="flex-grow: 1; flex-shrink: 1;">
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
    init_logging();

    let large_files = create_rw_signal(LargeFileSet(HashSet::new()));
    provide_context(large_files);

    mount_to_body(move || {
        view! {
            <I18nContextProvider>
                <ThemeProvider>
                    <GlobalStyle/>
                    <App/>
                </ThemeProvider>
            </I18nContextProvider>
        }
    })
}
