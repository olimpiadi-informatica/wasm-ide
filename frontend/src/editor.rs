use std::{
    future::Future,
    sync::{Arc, Mutex},
};

use async_channel::{unbounded, Receiver, Sender};
use common::{Language, WorkerLSResponse};
use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use thaw::{Button, ButtonAppearance, FileList, Theme, Upload};
use tracing::{debug, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::js_sys::Function;

use crate::{save, util::download, KeyboardMode};

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
    fn new(id: &str) -> CM6Editor;

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
    fn set_text(this: &CM6Editor, value: String);

    #[wasm_bindgen(method, js_name = "setKeymap")]
    fn set_keymap(this: &CM6Editor, kbh: &str);

    #[wasm_bindgen(method, js_name = "setLanguageServer")]
    fn set_language_server(this: &CM6Editor, send_message: Function) -> LSEventHandler;
}

pub struct EditorText {
    data: String,
    num_pending_changes: Arc<Mutex<usize>>,
    sender: Sender<()>,
    receiver: Receiver<()>,
}

impl EditorText {
    pub fn from_text(text: String) -> EditorText {
        let (sender, receiver) = unbounded();
        EditorText {
            data: text,
            num_pending_changes: Arc::new(Mutex::new(0)),
            sender,
            receiver,
        }
    }
    pub fn from_str(text: &str) -> EditorText {
        EditorText::from_text(text.to_string())
    }
    pub fn text(&self) -> &String {
        &self.data
    }
    pub fn await_all_changes(&self) -> impl Future<Output = ()> + 'static {
        let num_pending_changes = self.num_pending_changes.clone();
        let receiver = self.receiver.clone();
        async move {
            loop {
                if *num_pending_changes.lock().unwrap() == 0 {
                    return;
                }
                receiver.recv().await.expect("sender dropped");
            }
        }
    }
}

pub type LSRecv = Receiver<WorkerLSResponse>;
pub type LSSend = Box<dyn Fn(String)>;

#[component]
pub fn Editor(
    contents: RwSignal<EditorText, LocalStorage>,
    cache_key: &'static str,
    #[prop(into)] syntax: Signal<Option<Language>>,
    #[prop(into)] readonly: Signal<bool>,
    ctrl_enter: Box<dyn Fn()>,
    #[prop(into)] kb_mode: Signal<KeyboardMode>,
    ls_interface: Option<(LSRecv, LSSend)>,
) -> impl IntoView {
    let cm6 = RwSignal::new_local(None);

    let owner = Owner::current().unwrap();
    let onchange = move |_: JsValue| {
        contents.update_untracked(|val| {
            *val.num_pending_changes.lock().unwrap() += 1;
        });
        let owner = owner.clone();
        spawn_local(async move {
            TimeoutFuture::new(100).await;
            let mut do_update = false;
            contents.update_untracked(|val| {
                let mut v = val.num_pending_changes.lock().unwrap();
                if *v != 0 {
                    *v -= 1;
                    do_update = *v == 0;
                }
            });
            if !do_update {
                return;
            }
            cm6.with_untracked(|x: &Option<CM6Editor>| {
                let Some(cm6) = x else {
                    return;
                };
                let data = cm6.get_text();
                contents.update_untracked(|val| {
                    val.data = data;
                    debug!("onchange: {cache_key} {}", val.data.len());
                    owner.with(|| save(cache_key, val));
                })
            });
            let sender = contents.with_untracked(|c| c.sender.clone());
            for _ in 0..sender.receiver_count() {
                sender.send(()).await.expect("receiver dropped");
            }
        });
    };

    let id = format!("{cache_key}-editor");
    {
        let id = id.clone();
        queue_microtask(move || {
            let editor = CM6Editor::new(&id);
            editor.set_exec(Closure::wrap(ctrl_enter).into_js_value().unchecked_into());
            editor.set_onchange(
                Closure::<dyn Fn(_)>::new(onchange)
                    .into_js_value()
                    .unchecked_into(),
            );
            if let Some((receiver, send_worker_message)) = ls_interface {
                let fun = Closure::wrap(send_worker_message)
                    .into_js_value()
                    .unchecked_into();
                let ls = editor.set_language_server(fun);
                spawn_local(async move {
                    loop {
                        let msg = receiver.recv().await.unwrap();
                        match msg {
                            WorkerLSResponse::Started => ls.ready(),
                            WorkerLSResponse::Stopping => ls.stopping(),
                            WorkerLSResponse::Message(s) => ls.message(s),
                        }
                    }
                });
            }
            cm6.set(Some(editor));
        });
    }

    let theme = Theme::use_rw_theme();
    Effect::new(move |_| {
        cm6.with(|x| {
            let Some(cm6) = x else {
                return;
            };
            cm6.set_dark(theme.get().name != *"light");
        });
    });

    Effect::new(move |_| {
        cm6.with(|x| {
            let Some(cm6) = x else {
                return;
            };
            cm6.set_text(contents.with(|x| x.text().to_string()));
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
            let lang = match syntax.get() {
                None => {
                    return;
                }
                Some(Language::C) => "c",
                Some(Language::CPP) => "cpp",
                Some(Language::Python) => "python",
            };
            cm6.set_language(lang);
        });
    });

    Effect::new(move |_| {
        cm6.with(|x| {
            let Some(cm6) = x else {
                return;
            };
            match kb_mode.get() {
                KeyboardMode::Standard => cm6.set_keymap(""),
                KeyboardMode::Vim => cm6.set_keymap("vim"),
                KeyboardMode::Emacs => cm6.set_keymap("emacs"),
            }
        });
    });

    let do_download = move |_| {
        let contents = contents.read_untracked();
        let ext = syntax.get_untracked().map_or("txt", Language::ext);
        let name = format!("{cache_key}.{ext}");
        download(&name, contents.text().as_bytes());
    };

    let owner = Owner::current().unwrap();
    let do_upload = move |files: FileList| {
        let file = files.get(0).expect("0 files?");
        let owner = owner.clone();
        spawn_local(async move {
            let promise = file.text();
            let text = JsFuture::from(promise).await;
            match text {
                Ok(text) => {
                    let text =
                        EditorText::from_text(text.as_string().expect("did not read a string"));
                    owner.with(|| save(cache_key, &text));
                    contents.set(text)
                }
                Err(err) => warn!("could not read file: {err:?}"),
            }
        });
    };

    view! {
        <div id=id style="height: 100%; width: 100%; font-size: 1.2em; position: relative;">
            <div style="position: absolute; top: 0; right: 0; display: flex; flex-direction: row; z-index: 100; opacity: 0.5;">
                <Upload custom_request=do_upload>
                    <Button
                        appearance=ButtonAppearance::Transparent
                        icon=icondata::ChUpload
                        disabled=readonly
                    />
                </Upload>
                <Button
                    appearance=ButtonAppearance::Transparent
                    icon=icondata::ChDownload
                    on_click=do_download
                />
            </div>
        </div>
    }
}
