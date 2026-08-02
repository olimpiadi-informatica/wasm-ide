use leptos::{prelude::*, task::spawn_local};
use web_sys::{DragEvent, FileList, KeyboardEvent, SubmitEvent};

use crate::{
    backend,
    editor::{Editor, EditorController, LSRecv, LSSend},
    i18n::*,
    settings::KeyboardMode,
    util::Icon,
};

#[derive(Clone, Copy)]
pub struct EditorDirController {
    dir: Signal<Option<String>>,
    editor_ctrl: EditorController,
    files_revision: RwSignal<u64>,
}

impl EditorDirController {
    pub fn new(dir: Signal<Option<String>>) -> Self {
        let editor_ctrl = EditorController::new();
        let files_revision = RwSignal::new(0);
        Self {
            dir,
            editor_ctrl,
            files_revision,
        }
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

    pub fn open_filename(&self) -> Signal<Option<String>> {
        self.editor_ctrl.filename.into()
    }

    pub fn dir(&self) -> Signal<Option<String>> {
        self.dir
    }

    pub fn files_revision(&self) -> Signal<u64> {
        self.files_revision.into()
    }
}

#[component]
pub fn EditorDir(
    controller: EditorDirController,
    #[prop(into)] syntax: Signal<Option<String>>,
    #[prop(into)] readonly: Signal<bool>,
    ctrl_enter: Callback<()>,
    #[prop(into)] keyboard_mode: Signal<KeyboardMode>,
    ls_interface: Option<(LSRecv, LSSend)>,
) -> impl IntoView {
    let EditorDirController {
        dir,
        editor_ctrl,
        files_revision,
    } = controller;
    let tabs = RwSignal::new(Vec::new());
    let workspace_files = RwSignal::new(Vec::<(String, String)>::new());
    let rename_target = RwSignal::new(None::<String>);
    let rename_value = RwSignal::new(String::new());

    Effect::new(move || {
        let revision = files_revision.get();
        let dir_path = dir.get();
        spawn_local(async move {
            editor_ctrl.wait_sync().await;
            let entries = match &dir_path {
                Some(dir_path) => {
                    let dir = common::opfs::open_dir(dir_path, true).await;
                    dir.list_entries().await
                }
                None => Vec::new(),
            };
            if dir.get_untracked() != dir_path || files_revision.get_untracked() != revision {
                return;
            }

            let mut files = Vec::new();
            if let Some(dir_path) = &dir_path {
                let dir = common::opfs::open_dir(dir_path, true).await;
                for entry in &entries {
                    let data = dir.open_file(entry, false).await.read().await;
                    if let Ok(text) = String::from_utf8(data) {
                        files.push((format!("{dir_path}/{entry}"), text));
                    }
                }
            }

            let current = editor_ctrl.filename.get_untracked();
            let current_is_present = current
                .as_ref()
                .is_some_and(|current| files.iter().any(|(filename, _)| filename == current));
            if !current_is_present {
                editor_ctrl
                    .filename
                    .set(files.first().map(|(filename, _)| filename.clone()));
            }
            tabs.set(entries);
            workspace_files.set(files);
        });
    });

    let remove_file = move |file: &str| {
        let Some(dir) = dir.get_untracked() else {
            tracing::error!("Directory not set when trying to remove file");
            return;
        };
        let t = tabs.get_untracked();
        if t.len() == 1 {
            return;
        }
        tabs.update(|t| t.retain(|f| f != file));
        let file_path = dir + "/" + file;
        editor_ctrl.filename.update(|f| {
            if f.as_deref() == Some(&file_path) {
                *f = None;
            }
        });
        spawn_local(async move {
            let file_path = file_path;
            editor_ctrl.wait_sync().await;
            common::opfs::remove_entry(&file_path, false).await;
            files_revision.update(|revision| *revision += 1);
        });
    };

    let commit_rename = move || {
        let Some(old_name) = rename_target.get_untracked() else {
            return;
        };
        let new_name = rename_value.get_untracked();
        let Some(dir) = dir.get_untracked() else {
            rename_target.set(None);
            tracing::error!("Directory not set when trying to rename file");
            return;
        };
        if !valid_filename(&new_name)
            || tabs
                .get_untracked()
                .iter()
                .any(|file| file != &old_name && file == &new_name)
        {
            return;
        }

        rename_target.set(None);
        spawn_local(async move {
            let old_path = format!("{dir}/{old_name}");
            let new_path = format!("{dir}/{new_name}");
            if old_path == new_path {
                return;
            }
            controller.editor_ctrl.wait_sync().await;
            let old_file = common::opfs::open_file(&old_path, false).await;
            let data = old_file.read().await;
            let new_file = common::opfs::open_file(&new_path, true).await;
            new_file.write(&data).await;
            common::opfs::remove_entry(&old_path, false).await;
            tabs.update(|files| {
                if let Some(file) = files.iter_mut().find(|file| **file == old_name) {
                    *file = new_name.clone();
                }
            });
            controller.editor_ctrl.filename.update(|open| {
                if open.as_deref() == Some(&old_path) {
                    *open = Some(new_path);
                }
            });
            controller.files_revision.update(|revision| *revision += 1);
        });
    };

    let render_tab = move |file: String| {
        let file2 = file.clone();
        let file_path =
            Signal::derive(move || dir.get().map(|d| d + "/" + &file2).unwrap_or_default());
        let file3 = file.clone();
        let file4 = file.clone();
        let file5 = file.clone();
        let file6 = file.clone();
        let file7 = file.clone();
        let file8 = file.clone();
        let is_renaming = Signal::derive(move || rename_target.get().as_deref() == Some(&file4));
        let bad_filename = Signal::derive(move || {
            if rename_target.get().as_deref() != Some(&file5) {
                return false;
            }
            let value = rename_value.get();
            !valid_filename(&value)
                || tabs
                    .get()
                    .iter()
                    .any(|other| other != &file5 && other == &value)
        });

        view! {
            <li class:is-active=move || {
                editor_ctrl.filename.read().as_deref() == Some(&file_path.get())
            }>
                <Show
                    when=move || is_renaming.get()
                    fallback=move || {
                        let label = file6.clone();
                        let rename_name = file3.clone();
                        let target_name = file8.clone();
                        let close_name = file7.clone();
                        view! {
                            <a
                                on:click=move |_| {
                                    editor_ctrl.filename.set(Some(file_path.get_untracked()))
                                }
                                on:dblclick=move |_| {
                                    rename_value.set(rename_name.clone());
                                    rename_target.set(Some(target_name.clone()));
                                }
                            >
                                <span>{label}</span>
                                <Icon
                                    class:hover-red
                                    icon=icondata::IoClose
                                    class:is-clickable
                                    class:ml-2
                                    on:click=move |ev| {
                                        ev.stop_propagation();
                                        remove_file(&close_name);
                                    }
                                />

                            </a>
                        }
                    }
                >
                    <div
                        class:is-flex
                        class:is-align-items-center
                        class:is-column-gap-2
                        style:padding="0.5em 0.75em"
                    >
                        <input
                            class="input"
                            class:is-danger=bad_filename
                            type="text"
                            prop:value=move || rename_value.get()
                            on:input:target=move |ev| rename_value.set(ev.target().value())
                            on:blur=move |_| commit_rename()
                            on:keydown=move |ev: KeyboardEvent| {
                                match ev.key().as_str() {
                                    "Enter" => commit_rename(),
                                    "Escape" => rename_target.set(None),
                                    _ => {}
                                }
                            }
                        />
                        <Icon
                            class:hover-red
                            icon=icondata::IoClose
                            class:is-clickable
                            on:click=move |_| rename_target.set(None)
                        />
                    </div>
                </Show>
            </li>
        }
    };

    let import_files = move |files: FileList| {
        let Some(dir_path) = dir.get_untracked() else {
            tracing::error!("Directory not set when trying to import files");
            return;
        };
        spawn_local(async move {
            let mut file_to_open = None;
            for i in 0..files.length() {
                let file = files.get(i).expect("Failed to get file from FileList");
                let file_name = file.name();
                if !valid_filename(&file_name)
                    || tabs
                        .get_untracked()
                        .iter()
                        .any(|existing| existing == &file_name)
                {
                    continue;
                }

                let Ok(bytes) = file.bytes().await else {
                    tracing::error!("Failed to read imported file {file_name}");
                    continue;
                };
                let bytes = js_sys::Uint8Array::new(&bytes).to_vec();
                let file_path = format!("{dir_path}/{file_name}");
                let new_file = common::opfs::open_file(&file_path, true).await;
                new_file.write(&bytes).await;
                tabs.update(|t| t.push(file_name.clone()));
                if file_to_open.is_none() && std::str::from_utf8(&bytes).is_ok() {
                    file_to_open = Some(file_path);
                }
            }

            if let Some(file_path) = file_to_open {
                controller.editor_ctrl.filename.set(Some(file_path));
            }
            controller.files_revision.update(|revision| *revision += 1);
        });
    };

    let dragover_handler = move |ev: DragEvent| {
        ev.prevent_default();
    };

    let drop_handler = move |ev: DragEvent| {
        ev.prevent_default();
        let Some(data_transfer) = ev.data_transfer() else {
            return;
        };
        let Some(files) = data_transfer.files() else {
            return;
        };
        import_files(files);
    };

    view! {
        <div class:is-flex class:is-flex-direction-column style:height="100%">
            <div
                class:is-flex
                class:is-align-items-center
                class:is-justify-content-space-between
                on:dragover=dragover_handler
                on:drop=drop_handler
            >
                <div class:tabs class:is-boxed class:mb-0 style:width="fit-content">
                    <For
                        each=move || tabs.get().into_iter()
                        key=|file| file.clone()
                        children=render_tab
                    />
                </div>
                <AddFile controller tabs />
            </div>
            <div class:is-flex-grow-1 style:height="0">
                <Editor
                    controller=editor_ctrl
                    syntax=syntax
                    readonly=readonly
                    ctrl_enter=ctrl_enter
                    keyboard_mode=keyboard_mode
                    files=workspace_files
                    ls_interface=ls_interface
                />
            </div>
        </div>
    }
}

fn valid_filename(name: &str) -> bool {
    !name.is_empty() && !name.contains('/')
}

#[component]
fn AddFile(controller: EditorDirController, tabs: RwSignal<Vec<String>>) -> impl IntoView {
    let i18n = use_i18n();
    let open_modal = RwSignal::new(false);
    let filename = RwSignal::new(String::new());
    let extension = RwSignal::new(String::new());

    let add_file = move |ev: SubmitEvent| {
        ev.prevent_default();
        let value = filename.get_untracked();
        let ext = extension.get_untracked();
        let full_name = value + &ext;
        let Some(dir) = controller.dir.get_untracked() else {
            open_modal.set(false);
            tracing::error!("Directory not set when trying to add file");
            return;
        };
        if valid_filename(&full_name) && !tabs.get_untracked().iter().any(|f| f == &full_name) {
            let file = format!("{dir}/{full_name}");
            open_modal.set(false);
            spawn_local(async move {
                common::opfs::open_file(&file, true).await.write(&[]).await;
                controller.editor_ctrl.filename.set(Some(file));
                tabs.update(|t| t.push(full_name));
                controller.files_revision.update(|revision| *revision += 1);
            });
        }
    };

    let bad_filename = Signal::derive(move || {
        let value = filename.get();
        let full_name = value + &extension.get();
        !valid_filename(&full_name) || tabs.get().iter().any(|f| f == &full_name)
    });

    view! {
        <Icon
            icon=icondata::CgAddR
            class:is-clickable
            class:is-hidden=move || controller.dir.get().is_none()
            style:height="1.5em"
            style:width="1.5em"
            on:click=move |_| open_modal.set(true)
        />

        <div class:modal class:is-active=open_modal>
            <div class="modal-background" on:click=move |_| open_modal.set(false) />
            <div class="modal-card">
                <header class="modal-card-head">
                    <p class="modal-card-title">{t!(i18n, create_file_title)}</p>
                    <button
                        class="delete"
                        aria-label="close"
                        on:click=move |_| open_modal.set(false)
                    />
                </header>
                <section class="modal-card-body">
                    <form
                        class:is-flex
                        class:is-column-gap-2
                        class:is-align-items-center
                        class:mb-6
                        on:submit=add_file
                    >
                        <div class:select>
                            <select
                                prop:value=move || extension.get()
                                on:change:target=move |ev| extension.set(ev.target().value())
                            >
                                <For
                                    each=move || backend::languages()
                                    key=|lang| lang.name.clone()
                                    let:lang
                                >
                                    <option value=format!(
                                        ".{}",
                                        lang.extensions[0],
                                    )>{lang.name}</option>
                                </For>
                                <option value="">{t!(i18n, other)}</option>
                            </select>
                        </div>
                        <input
                            class="input"
                            class:is-danger=bad_filename
                            type="text"
                            placeholder="filename"
                            bind:value=filename
                        />
                        <button class="button is-primary" type="submit">
                            {t!(i18n, create_file)}
                        </button>
                    </form>
                </section>
            </div>
        </div>
    }
}
