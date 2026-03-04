use std::sync::Arc;

use common::Language;
use leptos::{prelude::*, task::spawn_local};

use crate::{
    editor::{Editor, EditorController, LSRecv, LSSend},
    settings::KeyboardMode,
    util::Icon,
};

pub struct EditorDirController {
    dir: Signal<Option<String>>,
    editor_ctrl: Arc<EditorController>,
}

impl EditorDirController {
    pub fn new(dir: Signal<Option<String>>) -> Self {
        let editor_ctrl = Arc::new(EditorController::new());
        Self { dir, editor_ctrl }
    }

    pub async fn wait_sync(&self) {
        self.editor_ctrl.wait_sync().await;
    }

    pub fn get_text(&self) -> String {
        self.editor_ctrl.get_text()
    }

    pub fn set_text(&self, text: &str) {
        self.editor_ctrl.set_text(text);
    }
}

#[component]
pub fn EditorDir(
    controller: Arc<EditorDirController>,
    #[prop(into)] syntax: Signal<Option<Language>>,
    #[prop(into)] readonly: Signal<bool>,
    ctrl_enter: Callback<()>,
    #[prop(into)] keyboard_mode: Signal<KeyboardMode>,
    ls_interface: Option<(LSRecv, LSSend)>,
) -> impl IntoView {
    let tabs = RwSignal::new(Vec::new());
    let open_modal = RwSignal::new(false);
    let filename = RwSignal::new(String::new());

    Effect::new({
        let controller = controller.clone();
        move || {
            let dir_path = controller.dir.get();
            let controller = controller.clone();
            spawn_local(async move {
                let entries = match dir_path {
                    Some(dir_path) => {
                        let dir = common::opfs::open_dir(&dir_path, true).await;
                        let entries = dir.list_entries().await;
                        controller
                            .editor_ctrl
                            .file_set(entries.first().map(|entry| dir_path + "/" + entry));
                        entries
                    }
                    None => Vec::new(),
                };
                tabs.try_update(|t| {
                    *t = entries;
                });
            });
        }
    });

    let render_tab = {
        let controller = controller.clone();
        move |file: String| {
            let ctrl1 = controller.clone();
            let ctrl2 = controller.clone();
            let controller = controller.clone();
            let file2 = file.clone();
            let file_path = Signal::derive(move || {
                controller
                    .dir
                    .get()
                    .map(|d| d + "/" + &file2)
                    .unwrap_or_default()
            });

            view! {
                <li class:is-active=move || {
                    ctrl1.editor_ctrl.file_get().as_deref() == Some(&file_path.get())
                }>
                    <a on:click=move |_| {
                        ctrl2.editor_ctrl.file_set(Some(file_path.get_untracked()))
                    }>{file}</a>
                </li>
            }
        }
    };

    let add_file = {
        let controller = controller.clone();
        move || {
            let value = filename.get();
            let name = if value.is_empty() { None } else { Some(value) };
            let Some(dir) = controller.dir.get_untracked() else {
                open_modal.set(false);
                tracing::error!("Directory not set when trying to add file");
                return;
            };
            if let Some(name) = name {
                let file = dir + "/" + &name;
                controller.editor_ctrl.file_set(Some(file.clone()));
                tabs.update(|t| t.push(name));
                open_modal.set(false);
            }
        }
    };

    let bad_filename = Signal::derive(move || {
        let value = filename.get();
        value.is_empty() || tabs.get().iter().any(|f| f == &value)
    });

    view! {
        <div class:is-flex class:is-flex-direction-column style:height="100%">
            <div class:modal class:is-active=open_modal>
                <div class="modal-background" on:click=move |_| open_modal.set(false) />
                <div class="modal-card">
                    <section class="modal-card-body">
                        <p>"Create file with name:"</p>
                        <input
                            class:input
                            class:is-danger=bad_filename
                            type="text"
                            placeholder="filename.cpp"
                            bind:value=filename
                        />
                    </section>
                    <footer class="modal-card-foot">
                        <div class="buttons">
                            <button class="button is-success" on:click=move |_| add_file()>
                                "Create file"
                            </button>
                            <button class="button" on:click=move |_| open_modal.set(false)>
                                "Cancel"
                            </button>
                        </div>
                    </footer>
                </div>
            </div>

            <div class:is-flex class:is-align-items-center class:is-justify-content-space-between>
                <div class:tabs class:is-boxed class:mb-0 style:width="fit-content">
                    <For
                        each=move || tabs.get().into_iter()
                        key=|file| file.clone()
                        children=render_tab
                    />
                </div>
                <Icon
                    icon=icondata::CgAddR
                    class:is-clickable
                    style:height="1.5em"
                    style:width="1.5em"
                    on:click=move |_| open_modal.set(true)
                />
            </div>
            <Editor
                controller=controller.editor_ctrl.clone()
                syntax=syntax
                readonly=readonly
                ctrl_enter=ctrl_enter
                keyboard_mode=keyboard_mode
                ls_interface=ls_interface
            />
        </div>
    }
}
