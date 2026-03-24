use common::WorkerLSResponse;
use futures_channel::mpsc::UnboundedReceiver;
use futures_util::StreamExt;
use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use tracing::{debug, info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::js_sys::Function;
use web_sys::{HtmlDivElement, HtmlElement, HtmlInputElement};

use crate::settings::{KeyboardMode, Theme, use_settings};
use crate::util::{Icon, download};

#[wasm_bindgen(raw_module = "./codemirror.js")]
extern "C" {
    type LSEventHandler;

    #[wasm_bindgen(method)]
    fn ready(this: &LSEventHandler);
    #[wasm_bindgen(method)]
    fn stopping(this: &LSEventHandler);
    #[wasm_bindgen(method)]
    fn message(this: &LSEventHandler, msg: String);

    type CM6Editor;

    #[wasm_bindgen(constructor)]
    fn new(id: &HtmlElement) -> CM6Editor;

    #[wasm_bindgen(method, js_name = "setLanguage")]
    fn set_language(this: &CM6Editor, lang: &str);

    #[wasm_bindgen(method, js_name = "setOnchange")]
    fn set_onchange(this: &CM6Editor, onchange: Function);

    #[wasm_bindgen(method, js_name = "setExec")]
    fn set_exec(this: &CM6Editor, exec: Function);

    #[wasm_bindgen(method, js_name = "setDark")]
    fn set_dark(this: &CM6Editor, dark: bool);

    #[wasm_bindgen(method, js_name = "setReadOnly")]
    fn set_readonly(this: &CM6Editor, readonly: bool);

    #[wasm_bindgen(method, js_name = "getText")]
    fn get_text(this: &CM6Editor) -> String;

    #[wasm_bindgen(method, js_name = "setText")]
    fn set_text(this: &CM6Editor, value: &str);

    #[wasm_bindgen(method, js_name = "setKeymap")]
    fn set_keymap(this: &CM6Editor, kbh: &str);

    #[wasm_bindgen(method, js_name = "setLanguageServer")]
    fn set_language_server(this: &CM6Editor, send_message: Function) -> LSEventHandler;
}

#[derive(Clone, Copy)]
pub struct EditorController {
    pub filename: RwSignal<Option<String>>,
    cm6: RwSignal<Option<CM6Editor>, LocalStorage>,
    pending_changes: RwSignal<bool>,
}

impl EditorController {
    pub fn new() -> EditorController {
        let filename = RwSignal::new(None);
        let cm6 = RwSignal::new_local(None);
        let pending_changes = RwSignal::new(false);
        EditorController {
            filename,
            cm6,
            pending_changes,
        }
    }

    pub async fn wait_sync(&self) {
        let mut pending_changes = self.pending_changes.to_stream();
        while pending_changes.next().await == Some(true) {}
    }

    pub fn get_text(&self) -> String {
        self.cm6
            .read_untracked()
            .as_ref()
            .expect("CM6 not initialized")
            .get_text()
    }

    pub fn set_text(&self, text: &str) {
        self.cm6
            .read_untracked()
            .as_ref()
            .expect("CM6 not initialized")
            .set_text(text)
    }
}

pub type LSRecv = UnboundedReceiver<WorkerLSResponse>;
pub type LSSend = Box<dyn Fn(String)>;

#[component]
pub fn Editor(
    controller: EditorController,
    #[prop(into)] syntax: Signal<Option<String>>,
    #[prop(into)] readonly: Signal<bool>,
    ctrl_enter: Callback<()>,
    #[prop(into)] keyboard_mode: Signal<KeyboardMode>,
    ls_interface: Option<(LSRecv, LSSend)>,
) -> impl IntoView {
    let EditorController {
        filename,
        cm6,
        pending_changes,
    } = controller;

    let open_filename = RwSignal::new(None::<String>);

    let readonly = Signal::derive(move || {
        readonly.get()
            || open_filename
                .get()
                .zip(filename.get())
                .is_none_or(|(a, b)| a != b)
    });

    let onchange = move |_: JsValue| {
        let old_pending = pending_changes
            .try_update(|v| std::mem::replace(v, true))
            .unwrap();
        if old_pending {
            return;
        }
        spawn_local(async move {
            TimeoutFuture::new(100).await;
            if let Some(name) = open_filename.get_untracked() {
                let text = controller.get_text();
                debug!("onchange: writing {} bytes", text.len());
                let file = common::opfs::open_file(&name, true).await;
                file.write(text.as_bytes()).await;
            }
            pending_changes.set(false);
        });
    };

    let node_ref = NodeRef::new();
    node_ref.on_load(move |div: HtmlDivElement| {
        let editor = CM6Editor::new(&div);
        editor.set_exec(
            Closure::wrap(Box::new(move || ctrl_enter.run(())) as Box<dyn Fn()>)
                .into_js_value()
                .unchecked_into(),
        );
        editor.set_onchange(
            Closure::<dyn Fn(_)>::new(onchange)
                .into_js_value()
                .unchecked_into(),
        );
        if let Some((mut receiver, send_worker_message)) = ls_interface {
            let fun = Closure::wrap(send_worker_message)
                .into_js_value()
                .unchecked_into();
            let ls = editor.set_language_server(fun);
            spawn_local(async move {
                loop {
                    let msg = receiver.next().await.unwrap();
                    match msg {
                        WorkerLSResponse::FetchingCompiler => {}
                        WorkerLSResponse::Started => ls.ready(),
                        WorkerLSResponse::Stopped => ls.stopping(),
                        WorkerLSResponse::Message(s) => ls.message(s),
                        WorkerLSResponse::Error(_) => ls.stopping(),
                    }
                }
            });
        }
        cm6.set(Some(editor));
    });

    Effect::new(move |_| {
        let name = filename.get();
        spawn_local(async move {
            let data = match &name {
                None => Vec::new(),
                Some(name) => {
                    let file = common::opfs::open_file(name, true).await;
                    file.read().await
                }
            };

            controller.wait_sync().await;

            if filename.get_untracked() != name {
                return;
            }

            let cm6 = cm6.read_untracked();
            let Some(cm6) = cm6.as_ref() else {
                return;
            };
            cm6.set_text(std::str::from_utf8(&data).unwrap());
            open_filename.set(name);
        });
    });

    let settings = use_settings();
    Effect::new(move |_| {
        cm6.with(|x| {
            let Some(cm6) = x else {
                return;
            };
            cm6.set_dark(settings.theme.get() != Theme::Light);
        });
    });

    Effect::new(move |_| {
        cm6.with(|x| {
            let Some(cm6) = x else {
                return;
            };
            cm6.set_readonly(readonly.get());
        });
    });

    Effect::new(move |_| {
        cm6.with(|x| {
            let Some(cm6) = x else {
                return;
            };
            let lang = syntax.get().unwrap_or_default();
            cm6.set_language(&lang);
        });
    });

    Effect::new(move |_| {
        cm6.with(|x| {
            let Some(cm6) = x else {
                return;
            };
            match keyboard_mode.get() {
                KeyboardMode::Standard => cm6.set_keymap(""),
                KeyboardMode::Vim => cm6.set_keymap("vim"),
                KeyboardMode::Emacs => cm6.set_keymap("emacs"),
            }
        });
    });

    let do_download = move |_| {
        let name = open_filename.read();
        let Some(name) = name.as_ref() else {
            info!("no file open, download cancelled");
            return;
        };
        let text = controller.get_text();
        let name = name.split('/').next_back().unwrap_or(name);
        download(name, text.as_bytes());
    };

    let upload_el = NodeRef::new();

    let do_upload = move |_| {
        let input: HtmlInputElement = upload_el.get().unwrap();
        let files = input.files().unwrap();
        let Some(file) = files.get(0) else {
            info!("file selection cancelled");
            return;
        };
        spawn_local(async move {
            let promise = file.text();
            let text = JsFuture::from(promise).await;
            match text {
                Ok(text) => {
                    controller.set_text(&text.as_string().expect("did not read a string"));
                }
                Err(err) => warn!("could not read file: {err:?}"),
            }
        });
    };

    view! {
        <div node_ref=node_ref class:is-height-100 class:is-size-6 class:is-relative>
            <div
                class:is-size-4
                class:is-opacity-50
                style:position="absolute"
                style:top="0"
                style:right="1.0rem"
                style:z-index="20"
                class:is-flex
                class:is-flex-direction-row
            >
                <Icon icon=icondata::ChDownload on:click=do_download class:is-clickable class:m-1 />
                <input type="file" class:is-hidden node_ref=upload_el on:change=do_upload />
                <Icon
                    icon=icondata::ChUpload
                    on:click=move |_| {
                        if !readonly.get() {
                            upload_el.get().unwrap().click()
                        }
                    }
                    class:is-clickable=move || !readonly.get()
                    class:is-opacity-50=move || readonly.get()
                    class:m-1
                />
            </div>
        </div>
    }
}
