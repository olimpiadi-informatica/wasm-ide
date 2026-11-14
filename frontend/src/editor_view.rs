use std::ops::Deref;

use common::{WorkerExecRequest, WorkerLSRequest, WorkerRequest};
use leptos::prelude::*;
use leptos_i18n::{t, t_display};
use leptos_use::{UseMouseReturn, UseWindowSizeReturn, use_mouse, use_window_size};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;

use crate::RunState;
use crate::editor::LSRecv;
use crate::editor_dir::{EditorDir, EditorDirController, EditorDirKind};
use crate::i18n::use_i18n;
use crate::output::OutputView;
use crate::settings::{InputMode, SettingsProvider, use_settings};
use crate::util::{Icon, get_input_mode};

#[derive(Clone, Copy, PartialEq, Eq)]
enum MobilePane {
    Code,
    Input,
    Output,
}

#[component]
pub fn EditorView(
    ls_receiver: LSRecv,
    send_worker_message: Callback<WorkerRequest>,
    code: EditorDirController,
    stdin: EditorDirController,
    ctrl_enter: Callback<()>,
    #[prop(into)] language: Signal<String>,
    #[prop(into)] code_readonly: Signal<bool>,
    #[prop(into)] input_readonly: Signal<bool>,
    #[prop(into)] disable_additional_input: Signal<bool>,
    state: RwSignal<RunState>,
) -> impl IntoView {
    let settings = use_settings();
    let SettingsProvider {
        editor_width_percent,
        input_height_percent,
        keyboard_mode,
        input_mode,
        ..
    } = settings;

    let additional_input = RwSignal::new(String::from(""));

    let add_input = move || {
        let mut extra = additional_input.get_untracked();
        if extra.is_empty() {
            return;
        }
        additional_input.set(String::new());
        let cur_stdin = stdin.get_text();
        if !cur_stdin.is_empty() && !cur_stdin.ends_with('\n') {
            extra = format!("\n{extra}");
        }
        if !extra.ends_with('\n') {
            extra = format!("{extra}\n");
        }
        stdin.set_text(&(cur_stdin + &extra));
        send_worker_message.run(WorkerExecRequest::StdinChunk(extra.into_bytes()).into());
    };

    let i18n = use_i18n();

    let additional_input_string =
        Signal::derive(move || t_display!(i18n, additional_input).to_string());

    let additional_input_line = {
        view! {
            <Show when=move || {
                get_input_mode(input_mode.get(), language.read().deref()) != InputMode::Batch
            }>
                <form
                    class="p-1.5 bg-base-200 border-b border-base-300 shrink-0"
                    on:submit=move |ev| {
                        ev.prevent_default();
                        add_input()
                    }
                >
                    <div class="join w-full">
                        <input
                            class="input input-bordered input-xs join-item flex-1 font-mono"
                            type="text"
                            placeholder=additional_input_string
                            disabled=disable_additional_input
                            bind:value=additional_input
                        />
                        <button
                            type="submit"
                            class="btn btn-success btn-xs join-item"
                            disabled=disable_additional_input
                        >
                            <Icon attr:class="w-3.5 h-3.5" icon=icondata::AiSendOutlined />
                        </button>
                    </div>
                </form>
            </Show>
        }
    };

    let is_resizing_col = RwSignal::new(false);
    let is_resizing_row = RwSignal::new(false);

    let stop_resize = move || {
        is_resizing_col.set(false);
        is_resizing_row.set(false);
    };

    window().set_onmouseup(Some(
        &Closure::<dyn Fn()>::new(stop_resize)
            .into_js_value()
            .unchecked_into(),
    ));

    window().set_ontouchend(Some(
        &Closure::<dyn Fn()>::new(stop_resize)
            .into_js_value()
            .unchecked_into(),
    ));

    let UseMouseReturn { x, y, .. } = use_mouse();
    let UseWindowSizeReturn { width, .. } = use_window_size();
    let right_col_ref = NodeRef::<leptos::html::Div>::new();
    let is_mobile = Memo::new(move |_| width.get() < 768.0);
    let mobile_pane = RwSignal::new(MobilePane::Code);
    let is_running = Signal::derive(move || state.with(|s| s.is_running()));

    Effect::new(move || {
        if is_resizing_col.get() {
            let perc = x.get() / width.get() * 100.0;
            settings.set_editor_width(perc as f32);
        }
        if is_resizing_row.get()
            && let Some(right_col) = right_col_ref.get_untracked()
        {
            let rect = right_col.get_bounding_client_rect();
            if rect.height() > 0.0 {
                let perc = (y.get() - rect.top()) / rect.height() * 100.0;
                settings.set_input_height(perc as f32);
            }
        }
    });

    let mobile_tab_btn = move |pane: MobilePane, label: ViewFn| {
        view! {
            <button
                type="button"
                class=move || {
                    if mobile_pane.get() == pane {
                        "btn btn-xs btn-primary gap-1.5"
                    } else {
                        "btn btn-xs btn-ghost gap-1.5 text-base-content/70"
                    }
                }
                on:click=move |_| mobile_pane.set(pane)
            >
                {label.run()}
                <Show when=move || pane == MobilePane::Output && is_running.get()>
                    <span class="loading loading-spinner loading-xs" />
                </Show>
            </button>
        }
    };

    view! {
        <div
            class:covers-page-col=is_resizing_col
            class:covers-page-row=is_resizing_row
        />
        <div class="flex flex-col flex-1 min-h-0 w-full overflow-hidden">
            <Show when=move || is_mobile.get()>
                <div class="grid grid-cols-3 gap-1 p-1 bg-base-200 border-b border-base-300 shrink-0">
                    {mobile_tab_btn(MobilePane::Code, ViewFn::from(move || t!(i18n, code)))}
                    {mobile_tab_btn(MobilePane::Input, ViewFn::from(move || t!(i18n, input)))}
                    {mobile_tab_btn(MobilePane::Output, ViewFn::from(move || t!(i18n, output)))}
                </div>
            </Show>
            <div class="flex flex-row flex-1 min-h-0 w-full overflow-hidden">
                <div
                    class=move || {
                        if is_mobile.get() {
                            if mobile_pane.get() == MobilePane::Code {
                                "flex-1 h-full min-h-0 min-w-0"
                            } else {
                                "hidden"
                            }
                        } else {
                            "h-full min-h-0 shrink-0"
                        }
                    }
                    style:width=move || {
                        if is_mobile.get() {
                            String::new()
                        } else {
                            format!("calc({}% - 3px)", editor_width_percent.get())
                        }
                    }
                >
                    <EditorDir
                        controller=code
                        kind=EditorDirKind::Code
                        syntax=language
                        readonly=code_readonly
                        ctrl_enter=ctrl_enter
                        keyboard_mode=keyboard_mode
                        ls_interface=Some((
                            ls_receiver,
                            Box::new(move |s| {
                                send_worker_message.run(WorkerLSRequest::Message(s).into())
                            }),
                        ))
                    />
                </div>
                <div
                    class=move || {
                        if is_mobile.get() {
                            "hidden"
                        } else {
                            "w-1.5 cursor-col-resize bg-base-300 hover:bg-primary active:bg-primary transition-colors shrink-0 select-none"
                        }
                    }
                    on:mousedown=move |e| {
                        is_resizing_col.set(true);
                        e.prevent_default();
                    }
                    on:touchstart=move |e| {
                        is_resizing_col.set(true);
                        e.prevent_default();
                    }
                />
                <div
                    node_ref=right_col_ref
                    class=move || {
                        if is_mobile.get() && mobile_pane.get() == MobilePane::Code {
                            "hidden"
                        } else {
                            "flex-1 min-w-0 h-full flex flex-col overflow-hidden"
                        }
                    }
                >
                    <div
                        class=move || {
                            if is_mobile.get() {
                                if mobile_pane.get() == MobilePane::Input {
                                    "w-full flex-1 min-h-0 flex flex-col"
                                } else {
                                    "hidden"
                                }
                            } else {
                                "w-full min-h-0 flex flex-col shrink-0"
                            }
                        }
                        style:height=move || {
                            if is_mobile.get() {
                                String::new()
                            } else {
                                format!("calc({}% - 3px)", input_height_percent.get())
                            }
                        }
                    >
                        {additional_input_line}
                        <div class="flex-1 min-h-0">
                            <EditorDir
                                controller=stdin
                                kind=EditorDirKind::Input
                                syntax=None
                                readonly=input_readonly
                                ctrl_enter=ctrl_enter
                                keyboard_mode=keyboard_mode
                                ls_interface=None
                            />
                        </div>
                    </div>
                    <div
                        class=move || {
                            if is_mobile.get() {
                                "hidden"
                            } else {
                                "h-1.5 cursor-row-resize bg-base-300 hover:bg-primary active:bg-primary transition-colors shrink-0 select-none"
                            }
                        }
                        on:mousedown=move |e| {
                            is_resizing_row.set(true);
                            e.prevent_default();
                        }
                        on:touchstart=move |e| {
                            is_resizing_row.set(true);
                            e.prevent_default();
                        }
                    />
                    <div class=move || {
                        if is_mobile.get() && mobile_pane.get() != MobilePane::Output {
                            "hidden"
                        } else {
                            "flex-1 min-h-0 w-full"
                        }
                    }>
                        <OutputView state />
                    </div>
                </div>
            </div>
        </div>
    }
}
