use std::str::Chars;

use anyhow::{bail, ensure, Result};
use icondata::Icon;
use leptos::prelude::*;
use leptos_use::signal_throttled;
use thaw::{
    Button, ButtonAppearance, ComponentRef, Divider, Popover, PopoverTrigger, Scrollbar,
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
) -> impl IntoView {
    let i18n = use_i18n();
    let scrollbar = ComponentRef::<ScrollbarRef>::new();

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

    let fragments = Signal::derive(move || ansi(&style_and_text.get().1));

    Effect::new(move |_| {
        fragments.track();
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
        <Scrollbar style="height: 18vh;" comp_ref=scrollbar>
            <pre style=style>
                <For each=move || fragments.get() key=|x| x.clone() let((style, text))>
                    <span style=style.style_str()>{text}</span>
                </For>
            </pre>
        </Scrollbar>
    }
}

#[component]
fn OutDiv(
    #[prop(into)] state: Signal<RunState>,
    get_data: fn(&Outcome) -> &Vec<u8>,
    icon: Icon,
    name: &'static str,
    tooltip: Signal<String>,
) -> impl IntoView {
    let disable_download = Signal::derive(move || {
        state.with(|s| !matches!(s, RunState::Complete(_) | RunState::Error(_, _)))
    });

    let do_download = move |_| {
        state.with(|s| {
            let RunState::Complete(outcome) = s else {
                warn!("requested download in invalid state");
                return;
            };
            let data = get_data(outcome);
            download(&format!("{name}.txt"), data);
        });
    };

    let open = RwSignal::new(false);

    let has_data = Signal::derive(move || {
        state.with(|s| match s {
            RunState::InProgress(o, _) | RunState::Error(_, o) | RunState::Complete(o) => {
                !get_data(o).is_empty()
            }
            _ => false,
        })
    });

    Effect::new(move |old: Option<bool>| {
        let old = old.unwrap_or(false);
        let s = state.read();
        let complete = matches!(*s, RunState::Complete(_) | RunState::Error(_, _));
        if complete && !old {
            open.set(has_data.get_untracked());
        }
        complete
    });

    let warn = Memo::new(move |_| has_data.get() && !open.get());

    view! {
        <div
            style="transition: flex 0.3s;"
            style:flex=move || if open.get() { "1" } else { "" }
            style:min-width=move || if open.get() { "0" } else { "" }
        >
            <div style="display: flex; align-items: center; justify-content: center; position: relative;">
                <div />
                <Popover>
                    <PopoverTrigger slot>
                        <div style="position: relative;">
                            <Button
                                icon
                                on_click=move |_| open.update(|v| *v = !*v)
                                appearance=ButtonAppearance::Subtle
                            />
                            <div
                                style="position: absolute; top: 4px; right: 4px; background-color: red; width: 8px; height: 8px; border-radius: 50%;"
                                hidden=move || !warn.get()
                            />
                        </div>
                    </PopoverTrigger>
                    {tooltip}
                </Popover>
                <Show when=move || open.get()>
                    <Button
                        disabled=disable_download
                        appearance=ButtonAppearance::Transparent
                        icon=icondata::ChDownload
                        on_click=do_download
                        attr:style="position: absolute; right: 8px;"
                    />
                </Show>
            </div>
            <Divider class="outdivider" />
            <Show when=move || open.get()>
                <OutDivInner state get_data />
            </Show>
        </div>
    }
}

#[component]
pub fn OutputView(state: RwSignal<RunState>) -> impl IntoView {
    let i18n = use_i18n();
    let state = signal_throttled(state, 100.0);

    let show_output_tooltip = Signal::derive(move || t_display!(i18n, show_output).to_string());
    let show_stderr_tooltip = Signal::derive(move || t_display!(i18n, show_stderr).to_string());
    let show_compileerr_tooltip =
        Signal::derive(move || t_display!(i18n, show_compileerr).to_string());

    view! {
        <div style="display: flex; justify-content: space-around; gap: 8px;">
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
