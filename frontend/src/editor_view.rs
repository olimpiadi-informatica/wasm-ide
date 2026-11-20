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
use crate::i18n::{t_string, use_i18n};
use crate::output::OutputView;
use crate::settings::{InputMode, SettingsProvider, use_settings};
use crate::util::{Icon, get_input_mode};

#[derive(Clone, Copy, PartialEq, Eq)]
enum MobilePane {
    Code,
    Input,
    Output,
    Statement,
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
    #[prop(into)] statement_url: Signal<Option<String>>,
    show_statement: RwSignal<bool>,
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

    Effect::new(move |prev_running: Option<bool>| {
        let running = is_running.get();
        if is_mobile.get_untracked() && running && !prev_running.unwrap_or(false) {
            mobile_pane.set(MobilePane::Output);
        }
        running
    });

    Effect::new(move |_| {
        if statement_url.get().is_none() && mobile_pane.get_untracked() == MobilePane::Statement {
            mobile_pane.set(MobilePane::Code);
        }
    });

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
                <div class=move || {
                    if statement_url.get().is_some() {
                        "grid grid-cols-4 gap-1 p-1 bg-base-200 border-b border-base-300 shrink-0"
                    } else {
                        "grid grid-cols-3 gap-1 p-1 bg-base-200 border-b border-base-300 shrink-0"
                    }
                }>
                    {mobile_tab_btn(MobilePane::Code, ViewFn::from(move || t!(i18n, code)))}
                    {mobile_tab_btn(MobilePane::Input, ViewFn::from(move || t!(i18n, input)))}
                    {mobile_tab_btn(MobilePane::Output, ViewFn::from(move || t!(i18n, output)))}
                    <Show when=move || statement_url.get().is_some()>
                        {mobile_tab_btn(
                            MobilePane::Statement,
                            ViewFn::from(move || t!(i18n, statement)),
                        )}
                    </Show>
                </div>
            </Show>
            <div class="flex flex-row flex-1 min-h-0 w-full overflow-hidden">
                <ShowLet
                    some=move || {
                        statement_url.get().filter(|_| {
                            if is_mobile.get() {
                                mobile_pane.get() == MobilePane::Statement
                            } else {
                                show_statement.get()
                            }
                        })
                    }
                    let:url
                >
                    <div class=move || {
                        if is_mobile.get() {
                            "flex-1 h-full min-h-0 min-w-0 flex flex-col bg-base-100"
                        } else {
                            "w-[40%] min-w-80 max-w-[50%] h-full min-h-0 shrink-0 border-r border-base-300 flex flex-col bg-base-100"
                        }
                    }>
                        <div class="flex items-center justify-between gap-2 px-3 py-1.5 bg-base-200 border-b border-base-300 shrink-0">
                            <span class="font-semibold text-xs flex items-center gap-1.5">
                                <Icon attr:class="w-3.5 h-3.5 text-primary" icon=icondata::BiBookOpenRegular />
                                {t!(i18n, statement)}
                            </span>
                            <div class="flex items-center gap-1">
                                <a
                                    href=url.clone()
                                    target="_blank"
                                    rel="noopener noreferrer"
                                    class="btn btn-xs btn-ghost gap-1"
                                >
                                    {t!(i18n, open_in_new_tab)}
                                    <Icon attr:class="w-3.5 h-3.5" icon=icondata::BiLinkExternalRegular />
                                </a>
                                <button
                                    type="button"
                                    class="btn btn-xs btn-ghost btn-square hidden md:inline-flex"
                                    on:click=move |_| show_statement.set(false)
                                >
                                    <Icon attr:class="w-3.5 h-3.5" icon=icondata::IoClose />
                                </button>
                            </div>
                        </div>
                        <StatementIframe url=url.clone() />
                    </div>
                </ShowLet>
                <div class=move || {
                    if is_mobile.get() && mobile_pane.get() == MobilePane::Statement {
                        "hidden"
                    } else {
                        "flex flex-row flex-1 min-w-0 h-full min-h-0 overflow-hidden"
                    }
                }>
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
        </div>
    }
}

#[component]
fn StatementIframe(url: String) -> impl IntoView {
    let i18n = use_i18n();
    let blob_url_store = StoredValue::new(None::<String>);
    on_cleanup(move || {
        if let Some(prev) = blob_url_store.get_value() {
            let _ = web_sys::Url::revoke_object_url(&prev);
        }
    });

    let pdf_resource = LocalResource::new(move || {
        let url = url.clone();
        async move {
            let res = gloo_net::http::Request::get(&url)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if !res.ok() {
                return Err(format!("HTTP {}", res.status()));
            }
            let bytes = res.binary().await.map_err(|e| e.to_string())?;
            let array = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes.as_slice()));
            let opts = web_sys::BlobPropertyBag::new();
            opts.set_type("application/pdf");
            let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&array, &opts)
                .map_err(|e| format!("{e:?}"))?;
            let obj_url =
                web_sys::Url::create_object_url_with_blob(&blob).map_err(|e| format!("{e:?}"))?;
            if let Some(prev) = blob_url_store.get_value() {
                let _ = web_sys::Url::revoke_object_url(&prev);
            }
            blob_url_store.set_value(Some(obj_url.clone()));
            Ok::<String, String>(obj_url)
        }
    });

    view! {
        {move || match pdf_resource.get() {
            None => view! {
                <div class="flex-1 flex items-center justify-center bg-base-100">
                    <span class="loading loading-spinner loading-md text-primary" />
                </div>
            }
                .into_any(),
            Some(Ok(obj_url)) => view! {
                <iframe
                    class="w-full flex-1 min-h-0 border-0 bg-white"
                    src=obj_url
                    title=move || t_string!(i18n, statement).to_string()
                />
            }
                .into_any(),
            Some(Err(err)) => view! {
                <div class="flex-1 flex flex-col items-center justify-center gap-2 p-4 text-sm text-error bg-base-100">
                    <span>{err}</span>
                </div>
            }
                .into_any(),
        }}
    }
}
