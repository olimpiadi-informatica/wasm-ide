use common::{WorkerExecRequest, WorkerLSRequest, WorkerRequest};
use leptos::prelude::*;
use leptos_i18n::t_display;
use leptos_use::{use_mouse, use_window_size, UseMouseReturn, UseWindowSizeReturn};
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::JsCast;

use crate::editor::{Editor, LSRecv};
use crate::i18n::use_i18n;
use crate::settings::{set_editor_width, use_settings, InputMode, SettingsProvider};
use crate::util::Icon;
use crate::EditorText;

#[component]
pub fn EditorView(
    ls_receiver: LSRecv,
    send_worker_message: Callback<WorkerRequest>,
    code: RwSignal<EditorText, LocalStorage>,
    stdin: RwSignal<EditorText, LocalStorage>,
    ctrl_enter: Callback<()>,
    #[prop(into)] code_readonly: Signal<bool>,
    #[prop(into)] input_readonly: Signal<bool>,
    #[prop(into)] disable_additional_input: Signal<bool>,
) -> impl IntoView {
    let SettingsProvider {
        editor_width_percent,
        language,
        keyboard_mode,
        input_mode,
        ..
    } = use_settings();

    let additional_input = RwSignal::new(String::from(""));

    let add_input = move || {
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
        stdin.set(EditorText::from_text(cur_stdin + &extra));
        send_worker_message.run(WorkerExecRequest::StdinChunk(extra.into_bytes()).into());
    };

    let i18n = use_i18n();

    let additional_input_string =
        Signal::derive(move || t_display!(i18n, additional_input).to_string());

    let additional_input_line = {
        view! {
            <div class:is-hidden=move || input_mode.get() == InputMode::Batch>
                <form on:submit=move |ev| {
                    ev.prevent_default();
                    add_input()
                }>
                    <div class:field class:has-addons>
                        <div class:control class:mb-2 style:width="100%">
                            <input
                                class="input"
                                type="text"
                                placeholder=additional_input_string
                                disabled=disable_additional_input
                                bind:value=additional_input
                            />

                        </div>
                        <div class:control>
                            <button class:button class:is-success disabled=disable_additional_input>
                                <Icon class:mr-1 class:my-1 icon=icondata::AiSendOutlined />
                            </button>
                        </div>
                    </div>
                </form>
            </div>
        }
    };

    let is_resizing = RwSignal::new(false);

    let stop_resize = move || {
        is_resizing.set(false);
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

    let UseMouseReturn { x, .. } = use_mouse();
    let UseWindowSizeReturn { width, .. } = use_window_size();

    Effect::new(move || {
        if is_resizing.get() {
            let perc = x.get() / width.get() * 100.0;
            set_editor_width(perc as f32);
        }
    });

    view! {
        <div class:covers-page=is_resizing />
        <div class:is-flex class:is-flex-direction-row class:is-flex-grow-1 style:height="0">
            <div style:width=move || format!("calc({}% - 0.35em)", editor_width_percent.get())>
                <Editor
                    contents=code
                    cache_key="code"
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
            <a
                style:width="0.7em"
                style:cursor="col-resize"
                on:mousedown=move |e| {
                    is_resizing.set(true);
                    e.prevent_default();
                }
                on:touchstart=move |e| {
                    is_resizing.set(true);
                    e.prevent_default();
                }
            />
            <div
                class:is-flex-grow-1
                style:width="0"
                class:is-flex
                class:is-flex-direction-column
                class:is-height-100
            >
                {additional_input_line}
                <div class:is-flex-grow-1 class:is-flex-shrink-1 style:min-height="0">
                    <Editor
                        contents=stdin
                        cache_key="stdin"
                        syntax=None
                        readonly=input_readonly
                        ctrl_enter=ctrl_enter
                        keyboard_mode=keyboard_mode
                        ls_interface=None
                    />
                </div>
            </div>
        </div>
    }
}
