use std::str::Chars;

use anyhow::{Result, bail, ensure};
use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_use::signal_throttled;
use tracing::warn;
use web_sys::ScrollToOptions;

use crate::i18n::*;
use crate::util::{Icon, copy_to_clipboard, download};
use crate::{Outcome, RunState, StateExec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputTab {
    Stdout,
    Stderr,
    Compilation,
}

impl OutputTab {
    fn get_data(self, outcome: &Outcome) -> &Vec<u8> {
        match self {
            OutputTab::Stdout => &outcome.stdout,
            OutputTab::Stderr => &outcome.stderr,
            OutputTab::Compilation => &outcome.compile_stderr,
        }
    }

    fn download_name(self) -> &'static str {
        match self {
            OutputTab::Stdout => "stdout.txt",
            OutputTab::Stderr => "stderr.txt",
            OutputTab::Compilation => "compilation.txt",
        }
    }
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
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
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum State {
            N0,
            N38,
            N38_5,
        }
        use State::*;

        ensure!(
            iter.next() == Some('['),
            "expected '[' at start of ANSI sequence"
        );
        let mut num = 0;
        let mut state = State::N0;
        for c in iter {
            if c.is_ascii_digit() {
                num = num * 10 + (c as u8 - b'0') as usize;
            } else if c == 'm' || c == ';' {
                match (state, num) {
                    (N0, 0) => *style = Style::default(),
                    (N0, 1) => style.bold = true,
                    (N0, 30) => style.fg = Some("var(--color-base-content)"),
                    (N0, 31) => style.fg = Some("oklch(0.60 0.22 25)"),
                    (N0, 32) => style.fg = Some("oklch(0.55 0.18 145)"),
                    (N0, 33) => style.fg = Some("oklch(0.62 0.17 75)"),
                    (N0, 34) => style.fg = Some("oklch(0.58 0.20 255)"),
                    (N0, 35) => style.fg = Some("oklch(0.58 0.22 320)"),
                    (N0, 36) => style.fg = Some("oklch(0.58 0.15 210)"),
                    (N0, 37) => style.fg = Some("var(--color-base-content)"),
                    (N0, 38) => state = N38,
                    (N38, 5) => state = N38_5,
                    (N38_5, c @ 0..16) => {
                        style.fg = Some(match c {
                            0 | 7 | 15 => "var(--color-base-content)",
                            1 => "oklch(0.60 0.22 25)",
                            2 => "oklch(0.55 0.18 145)",
                            3 => "oklch(0.62 0.17 75)",
                            4 => "oklch(0.58 0.20 255)",
                            5 => "oklch(0.58 0.22 320)",
                            6 => "oklch(0.58 0.15 210)",
                            8 => "oklch(0.55 0.02 250)",
                            9 => "oklch(0.66 0.20 25)",
                            10 => "oklch(0.64 0.18 145)",
                            11 => "oklch(0.68 0.16 80)",
                            12 => "oklch(0.66 0.18 250)",
                            13 => "oklch(0.66 0.20 320)",
                            14 => "oklch(0.66 0.14 210)",
                            _ => unreachable!(),
                        });
                        state = N0;
                    }
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
        if let Some(last) = fragments.last_mut()
            && last.0 == style
        {
            last.1.push(c);
            continue;
        }
        fragments.push((style, c.to_string()));
    }

    fragments
}

#[component]
pub fn OutputView(state: RwSignal<RunState>) -> impl IntoView {
    let i18n = use_i18n();
    let throttled_state: Signal<RunState> = signal_throttled(state, 80.0);

    let active_tab = RwSignal::new(OutputTab::Stdout);
    let user_selected_tab = RwSignal::new(false);
    let copied = RwSignal::new(false);

    let has_stdout = Memo::new(move |_| match &throttled_state.read().exec {
        StateExec::Processing { outcome, .. } | StateExec::Complete { outcome, .. } => {
            !outcome.stdout.is_empty()
        }
        _ => false,
    });
    let has_stderr = Memo::new(move |_| match &throttled_state.read().exec {
        StateExec::Processing { outcome, .. } | StateExec::Complete { outcome, .. } => {
            !outcome.stderr.is_empty()
        }
        _ => false,
    });
    let has_compile = Memo::new(move |_| match &throttled_state.read().exec {
        StateExec::Processing { outcome, .. } | StateExec::Complete { outcome, .. } => {
            !outcome.compile_stderr.is_empty()
        }
        _ => false,
    });

    // Reset manual tab override when a new execution starts, and auto-select the most relevant tab
    // as output streams in.
    Effect::new(move |was_running: Option<bool>| {
        let is_running = state.with(|s| matches!(s.exec, StateExec::Processing { .. }));
        if is_running && !was_running.unwrap_or(false) {
            user_selected_tab.set(false);
        }
        let s_out = has_stdout.get();
        let s_err = has_stderr.get();
        let s_cmp = has_compile.get();
        if !user_selected_tab.get_untracked() {
            if s_out {
                active_tab.set(OutputTab::Stdout);
            } else if s_err {
                active_tab.set(OutputTab::Stderr);
            } else if s_cmp {
                active_tab.set(OutputTab::Compilation);
            }
        }
        is_running
    });

    let did_execute = Signal::derive(move || {
        matches!(
            throttled_state.read().exec,
            StateExec::Processing { .. } | StateExec::Complete { .. },
        )
    });

    let exec_error = Signal::derive(move || match &state.read().exec {
        StateExec::Complete {
            error: Some(err), ..
        } => Some(err.clone()),
        _ => None,
    });

    let clear_exec_error = move |_| {
        if let StateExec::Complete { error, .. } = &mut state.write().exec {
            *error = None;
        }
    };

    let text = Signal::derive(move || {
        let tab = active_tab.get();
        match &throttled_state.read().exec {
            StateExec::Processing { outcome, .. } | StateExec::Complete { outcome, .. } => {
                output_for_display(tab.get_data(outcome))
            }
            _ => t_display!(i18n, not_yet_executed).to_string(),
        }
    });

    let active_has_data = Signal::derive(move || match active_tab.get() {
        OutputTab::Stdout => has_stdout.get(),
        OutputTab::Stderr => has_stderr.get(),
        OutputTab::Compilation => has_compile.get(),
    });

    let fragments = Signal::derive(move || ansi(&text.get()));

    let do_copy = move |_| {
        let tab = active_tab.get_untracked();
        let state = state.read_untracked();
        let (StateExec::Processing { outcome, .. } | StateExec::Complete { outcome, .. }) =
            &state.exec
        else {
            return;
        };
        let raw = String::from_utf8_lossy(tab.get_data(outcome));
        copy_to_clipboard(&raw);
        copied.set(true);
        spawn_local(async move {
            TimeoutFuture::new(1500).await;
            copied.set(false);
        });
    };

    let do_download = move |_| {
        let tab = active_tab.get_untracked();
        let state = state.read_untracked();
        let (StateExec::Processing { outcome, .. } | StateExec::Complete { outcome, .. }) =
            &state.exec
        else {
            warn!("requested download in invalid state");
            return;
        };
        let data = tab.get_data(outcome);
        download(tab.download_name(), data);
    };

    let content_ref = NodeRef::<leptos::html::Pre>::new();

    Effect::new(move |_| {
        fragments.track();
        queue_microtask(move || {
            let scroll_options = ScrollToOptions::new();
            scroll_options.set_behavior(web_sys::ScrollBehavior::Smooth);
            if let Some(content) = content_ref.get_untracked() {
                let height = content.scroll_height();
                scroll_options.set_top(height as f64);
                content.scroll_to_with_scroll_to_options(&scroll_options);
            }
        });
    });

    let select_tab = move |tab: OutputTab| {
        move |_| {
            active_tab.set(tab);
            user_selected_tab.set(true);
        }
    };

    view! {
        <div class="flex flex-col h-full w-full min-h-0 bg-base-100">
            <div class="flex items-center justify-between bg-base-200 border-b border-base-300 min-h-9 px-1 gap-1 shrink-0">
                <div class="flex items-center overflow-x-auto no-scrollbar min-w-0 flex-1">
                    <button
                        type="button"
                        class=move || {
                            if active_tab.get() == OutputTab::Stdout {
                                "flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium bg-base-100 text-base-content border-t-2 border-t-primary border-r border-base-300 cursor-pointer shrink-0"
                            } else {
                                "flex items-center gap-1.5 px-3 py-1.5 text-xs text-base-content/70 hover:text-base-content hover:bg-base-100/50 border-t-2 border-t-transparent border-r border-base-300 cursor-pointer shrink-0 transition-colors"
                            }
                        }
                        on:click=select_tab(OutputTab::Stdout)
                    >
                        <Icon attr:class="w-3.5 h-3.5" icon=icondata::VsOutput />
                        <span>{t!(i18n, output)}</span>
                        <Show when=move || has_stdout.get()>
                            <span class="w-2 h-2 rounded-full bg-info" />
                        </Show>
                    </button>

                    <button
                        type="button"
                        class=move || {
                            if active_tab.get() == OutputTab::Stderr {
                                "flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium bg-base-100 text-base-content border-t-2 border-t-primary border-r border-base-300 cursor-pointer shrink-0"
                            } else {
                                "flex items-center gap-1.5 px-3 py-1.5 text-xs text-base-content/70 hover:text-base-content hover:bg-base-100/50 border-t-2 border-t-transparent border-r border-base-300 cursor-pointer shrink-0 transition-colors"
                            }
                        }
                        on:click=select_tab(OutputTab::Stderr)
                    >
                        <Icon attr:class="w-3.5 h-3.5" icon=icondata::BiErrorSolid />
                        <span>{t!(i18n, stderr)}</span>
                        <Show when=move || has_stderr.get()>
                            <span class="w-2 h-2 rounded-full bg-error" />
                        </Show>
                    </button>

                    <button
                        type="button"
                        class=move || {
                            if active_tab.get() == OutputTab::Compilation {
                                "flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium bg-base-100 text-base-content border-t-2 border-t-primary border-r border-base-300 cursor-pointer shrink-0"
                            } else {
                                "flex items-center gap-1.5 px-3 py-1.5 text-xs text-base-content/70 hover:text-base-content hover:bg-base-100/50 border-t-2 border-t-transparent border-r border-base-300 cursor-pointer shrink-0 transition-colors"
                            }
                        }
                        on:click=select_tab(OutputTab::Compilation)
                    >
                        <Icon attr:class="w-3.5 h-3.5" icon=icondata::BiCommentErrorSolid />
                        <span>{t!(i18n, compileerr)}</span>
                        <Show when=move || has_compile.get()>
                            <span class="w-2 h-2 rounded-full bg-warning" />
                        </Show>
                    </button>
                </div>

                <div class="flex items-center gap-0.5 shrink-0 pr-1">
                    <button
                        type="button"
                        class="btn btn-ghost btn-xs btn-square"
                        title=move || {
                            if copied.get() {
                                t_string!(i18n, copied).to_string()
                            } else {
                                t_string!(i18n, copy_to_clipboard).to_string()
                            }
                        }
                        disabled=move || !active_has_data.get()
                        on:click=do_copy
                    >
                        <Icon
                            attr:class=move || {
                                if copied.get() { "w-3.5 h-3.5 text-success" } else { "w-3.5 h-3.5" }
                            }
                            icon=Signal::derive(move || {
                                if copied.get() { icondata::BiCheckRegular } else { icondata::LuCopy }
                            })
                        />
                    </button>
                    <button
                        type="button"
                        class="btn btn-ghost btn-xs btn-square"
                        title=move || t_string!(i18n, download_file)
                        disabled=move || !active_has_data.get()
                        on:click=do_download
                    >
                        <Icon attr:class="w-3.5 h-3.5" icon=icondata::ChDownload />
                    </button>
                </div>
            </div>

            <ShowLet some=exec_error let:err>
                <div class="alert alert-error rounded-none py-1.5 px-3 text-xs flex items-center justify-between gap-2 shrink-0 border-b border-error/30">
                    <div class="flex items-center gap-2 min-w-0">
                        <Icon attr:class="w-4 h-4 shrink-0" icon=icondata::BiErrorCircleSolid />
                        <span class="font-mono whitespace-pre-wrap break-all">{err}</span>
                    </div>
                    <button
                        type="button"
                        class="btn btn-ghost btn-xs btn-circle shrink-0"
                        title=move || t_string!(i18n, hide_error)
                        on:click=clear_exec_error
                    >
                        <Icon attr:class="w-3.5 h-3.5" icon=icondata::IoClose />
                    </button>
                </div>
            </ShowLet>

            <pre
                class=move || {
                    if did_execute.get() {
                        "flex-1 min-h-0 w-full overflow-auto p-3 font-mono text-xs leading-relaxed whitespace-pre-wrap break-words m-0"
                    } else {
                        "flex-1 min-h-0 w-full overflow-auto p-3 font-mono text-xs leading-relaxed text-base-content/40 italic m-0"
                    }
                }
                node_ref=content_ref
            >
                <For each=move || fragments.get() key=|x| x.clone() let((style, text))>
                    <span style=style.style_str()>{text}</span>
                </For>
            </pre>
        </div>
    }
}
