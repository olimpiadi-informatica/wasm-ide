use std::ops::Deref;
use std::str::Chars;

use anyhow::{bail, ensure, Result};
use leptos::prelude::*;
use leptos_use::{signal_throttled, use_mouse_in_element, UseMouseInElementReturn};
use tracing::warn;
use web_sys::ScrollToOptions;

use crate::i18n::*;
use crate::util::{download, Icon};
use crate::{Outcome, RunState, StateExec};

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
    name: &'static str,
    #[prop(into)] visible: Signal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();

    let did_execute = Signal::derive(move || {
        matches!(
            &*state.read(),
            RunState::Ready {
                exec: StateExec::Processing { .. } | StateExec::Complete { .. },
                ..
            }
        )
    });

    let text = Signal::derive(move || match state.read().deref() {
        RunState::Ready {
            exec: StateExec::Processing { outcome, .. } | StateExec::Complete { outcome, .. },
            ..
        } => output_for_display(get_data(outcome)),
        _ => t_display!(i18n, not_yet_executed).to_string(),
    });

    let fragments = Signal::derive(move || ansi(&text.get()));

    let disable_download = Signal::derive(move || {
        !matches!(
            state.read().deref(),
            RunState::Ready {
                exec: StateExec::Complete { .. },
                ..
            }
        )
    });

    let do_download = move |_| {
        let state = state.read_untracked();
        let RunState::Ready {
            exec: StateExec::Complete { outcome, .. },
            ..
        } = state.deref()
        else {
            warn!("requested download in invalid state");
            return;
        };
        let data = get_data(outcome);
        download(&format!("{name}.txt"), data);
    };

    let content = NodeRef::<leptos::html::Pre>::new();

    Effect::new(move |_| {
        fragments.track();
        // We want this to run *after* the DOM updates.
        queue_microtask(move || {
            let scroll_options = ScrollToOptions::new();
            scroll_options.set_behavior(web_sys::ScrollBehavior::Smooth);
            if let Some(content) = content.get_untracked() {
                let height = content.scroll_height();
                scroll_options.set_top(height as f64);
                content.scroll_to_with_scroll_to_options(&scroll_options);
            }
        });
    });

    view! {
        <div style:height="18vh" class:is-relative class:is-hidden=move || !visible.get()>
            <Show when=move || !disable_download.get()>
                <div
                    class:is-size-4
                    class:is-opacity-50
                    style:position="absolute"
                    style:top="0"
                    style:right="0.5em"
                    style:z-index="50"
                >
                    <Icon
                        icon=icondata::ChDownload
                        on:click=do_download
                        class:is-clickable
                        class:m-1
                    />
                </div>
            </Show>
            <pre
                style:width="100%"
                style:max-height="100%"
                class:has-text-left
                class:faint-text=move || !did_execute.get()
                node_ref=content
            >
                <For each=move || fragments.get() key=|x| x.clone() let((style, text))>
                    <span style=style.style_str()>{text}</span>
                </For>
            </pre>
        </div>
    }
}

#[component]
fn OutDiv(
    #[prop(into)] state: Signal<RunState>,
    get_data: fn(&Outcome) -> &Vec<u8>,
    icon: icondata::Icon,
    name: &'static str,
    tooltip: Signal<String>,
) -> impl IntoView {
    let open = RwSignal::new(false);

    let has_data = Signal::derive(move || match state.read().deref() {
        RunState::Ready {
            exec: StateExec::Processing { outcome, .. } | StateExec::Complete { outcome, .. },
            ..
        } => !get_data(outcome).is_empty(),
        _ => false,
    });

    Effect::new(move |old: Option<bool>| {
        // TODO(veluca): open the output early for non-batch input.
        let complete = matches!(
            state.read().deref(),
            RunState::Ready {
                exec: StateExec::Complete { .. },
                ..
            }
        );
        if complete && !old.unwrap_or(false) {
            open.set(has_data.get_untracked());
        }
        complete
    });

    let warn = Memo::new(move |_| has_data.get() && !open.get());
    let icon_ref = NodeRef::new();
    let UseMouseInElementReturn { is_outside, .. } = use_mouse_in_element(icon_ref);

    view! {
        <div
            style:transition="flex 0.3s 0ms"
            style:flex=move || if open.get() { "1" } else { "" }
            style:min-width=move || if open.get() { "15rem" } else { "" }
        >
            <div
                class:is-flex
                class:is-align-items-center
                class:is-justify-content-center
                class:m-1
                class:mt-2
            >
                <div class:is-relative node_ref=icon_ref class:is-clickable>
                    <div class:is-hidden=move || is_outside.get() class:box class:output-tooltip>
                        {tooltip}
                    </div>
                    <div
                        style:position="absolute"
                        style:top="0"
                        style:right="0"
                        style:width="0.5em"
                        style:height="0.5em"
                        style:border-radius="50%"
                        style:background-color="var(--bulma-danger)"
                        hidden=move || !warn.get()
                    />
                    <Icon class:icon on:click=move |_| open.update(|v| *v = !*v) icon=icon />
                </div>
            </div>
            <OutDivInner state get_data name visible=open />
        </div>
    }
}

#[component]
pub fn OutputView(state: RwSignal<RunState>) -> impl IntoView {
    let i18n = use_i18n();
    let state = signal_throttled(state, 100.0);

    let show_output_tooltip = Signal::derive(move || t_display!(i18n, output).to_string());
    let show_stderr_tooltip = Signal::derive(move || t_display!(i18n, stderr).to_string());
    let show_compileerr_tooltip = Signal::derive(move || t_display!(i18n, compileerr).to_string());

    view! {
        <div
            class:is-flex
            class:is-justify-content-space-around
            style:gap="1em"
            style:padding="0 1em 0 1em"
        >
            <OutDiv
                state
                get_data=|outcome| &outcome.stdout
                icon=icondata::VsOutput
                name="stdout"
                tooltip=show_output_tooltip
            />
            <OutDiv
                state
                get_data=|outcome| &outcome.stderr
                icon=icondata::BiErrorSolid
                name="stderr"
                tooltip=show_stderr_tooltip
            />
            <OutDiv
                state
                get_data=|outcome| &outcome.compile_stderr
                icon=icondata::BiCommentErrorSolid
                name="compilation"
                tooltip=show_compileerr_tooltip
            />
        </div>
    }
}
