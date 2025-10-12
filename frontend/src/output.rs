use std::str::Chars;

use anyhow::{bail, ensure, Result};
use icondata::Icon;
use leptos::prelude::*;
use leptos_use::signal_throttled;
use thaw::{
    Button, ButtonAppearance, ComponentRef, Divider, Icon, Popover, PopoverTrigger, Scrollbar,
    ScrollbarRef,
};
use tracing::warn;
use web_sys::ScrollToOptions;

use crate::{i18n::*, util::download, Outcome, RunState};

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
    icon: Icon,
    filename: &'static str,
) -> impl IntoView {
    let i18n = use_i18n();
    let scrollbar = ComponentRef::<ScrollbarRef>::new();

    let disable_download = Signal::derive(move || {
        state.with(|s| !matches!(s, RunState::Complete(_) | RunState::Error(_, _)))
    });

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

    Effect::new(move |_| {
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

    let do_download = move |_| {
        state.with(|s| {
            let RunState::Complete(outcome) = s else {
                warn!("requested download in invalid state");
                return;
            };
            let data = get_data(outcome);
            download(filename, data);
        });
    };

    view! {
        <div style="flex-grow: 1; flex-basis: 0; flex-shrink: 1; min-width: 0; text-align: center;">
            <div style="display: flex; flex-direction: row; align-items: center; justify-content: space-between; padding-top: 4px;">
                <div />
                <Icon icon style="font-size: 20px; width: 20px; height: 20px;" />
                <Button
                    disabled=disable_download
                    appearance=ButtonAppearance::Transparent
                    icon=icondata::ChDownload
                    on_click=do_download
                    attr:style="margin-right: 8px;"
                />
            </div>
            <Divider class="outdivider" />
            <Scrollbar style="height: 18vh;" comp_ref=scrollbar>
                <pre style=style>
                    <For each=move || fragments.get() key=|x| x.clone() let((style, text))>
                        <span style=style.style_str()>{text}</span>
                    </For>
                </pre>
            </Scrollbar>
        </div>
    }
}

#[component]
fn OutDiv(
    #[prop(into)] state: Signal<RunState>,
    #[prop(into)] display: Signal<bool>,
    get_data: fn(&Outcome) -> &Vec<u8>,
    icon: Icon,
    filename: &'static str,
) -> impl IntoView {
    view! {
        <Show when=move || display.get()>
            <OutDivInner state get_data icon filename />
        </Show>
    }
}

#[component]
pub fn OutputView(
    state: RwSignal<RunState>,
    #[prop(into)] show_stdout: Signal<bool>,
    #[prop(into)] show_stderr: Signal<bool>,
    #[prop(into)] show_compilation: Signal<bool>,
) -> impl IntoView {
    let state = signal_throttled(state, 100.0);
    let when = move || show_stdout.get() || show_stderr.get() || show_compilation.get();
    view! {
        <Show when>
            <div style="display: flex; flex-direction: row;">
                <OutDiv
                    state
                    display=show_stdout
                    get_data=|outcome| &outcome.stdout
                    icon=icondata::VsOutput
                    filename="stdout.txt"
                />
                <OutDiv
                    state
                    display=show_stderr
                    get_data=|outcome| &outcome.stderr
                    icon=icondata::BiErrorSolid
                    filename="stderr.txt"
                />
                <OutDiv
                    state
                    display=show_compilation
                    get_data=|outcome| &outcome.compile_stderr
                    icon=icondata::BiCommentErrorSolid
                    filename="compilation.txt"
                />
            </div>
        </Show>
    }
}

#[component]
pub fn OutputControl(
    signal: RwSignal<bool>,
    icon: Icon,
    tooltip: Signal<String>,
    #[prop(into)] color: String,
) -> impl IntoView {
    let appearance = Signal::derive(move || {
        if signal.get() {
            ButtonAppearance::Secondary
        } else {
            ButtonAppearance::Subtle
        }
    });
    view! {
        <Popover>
            <PopoverTrigger slot>
                <Button class=color icon on_click=move |_| signal.update(|x| *x = !*x) appearance />
            </PopoverTrigger>
            {tooltip}
        </Popover>
    }
}
