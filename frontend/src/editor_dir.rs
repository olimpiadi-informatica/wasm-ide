use gloo_timers::future::TimeoutFuture;
use leptos::{prelude::*, task::spawn_local};
use tracing::info;
use web_sys::{DragEvent, FileList, HtmlInputElement, KeyboardEvent};

use crate::{
    backend,
    editor::{Editor, EditorController, LSRecv, LSSend},
    i18n::*,
    settings::KeyboardMode,
    util::{Icon, copy_to_clipboard, download},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditorDirKind {
    Code,
    Input,
}

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

pub fn select_best_code_file(
    entries: &[String],
    lang_name: &str,
    preferred_stem: Option<&str>,
) -> Option<String> {
    let lang_exts = backend::languages()
        .into_iter()
        .find(|l| l.name == lang_name)
        .map(|l| l.extensions)
        .unwrap_or_default();
    let matches_lang = |name: &str| {
        let ext = name.rsplit('.').next().unwrap_or("");
        lang_exts.iter().any(|e| e == ext)
    };
    let is_grader = |name: &str| {
        let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
        stem == "grader"
    };

    if let Some(stem) = preferred_stem
        && let Some(found) = entries.iter().find(|name| {
            let file_stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
            file_stem == stem && matches_lang(name)
        })
    {
        return Some(found.clone());
    }
    if let Some(found) = entries.iter().find(|name| {
        let file_stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
        file_stem == "main" && matches_lang(name)
    }) {
        return Some(found.clone());
    }
    if let Some(found) = entries
        .iter()
        .find(|name| matches_lang(name) && !is_grader(name))
    {
        return Some(found.clone());
    }
    entries.iter().find(|name| matches_lang(name)).cloned()
}

fn suggest_filename(kind: EditorDirKind, syntax: Option<&str>, existing: &[String]) -> String {
    match kind {
        EditorDirKind::Input => {
            if !existing.iter().any(|f| f == "input.txt") {
                return "input.txt".to_owned();
            }
            for i in 1.. {
                let candidate = format!("input{i}.txt");
                if !existing.iter().any(|f| f == &candidate) {
                    return candidate;
                }
            }
            unreachable!()
        }
        EditorDirKind::Code => {
            let ext = syntax
                .and_then(|lang_name| {
                    backend::languages()
                        .into_iter()
                        .find(|l| l.name == lang_name)
                        .and_then(|l| l.extensions.first().cloned())
                })
                .unwrap_or_else(|| "cpp".to_owned());
            let main_candidate = format!("main.{ext}");
            if !existing.iter().any(|f| f == &main_candidate) {
                return main_candidate;
            }
            for i in 2.. {
                let candidate = format!("file{i}.{ext}");
                if !existing.iter().any(|f| f == &candidate) {
                    return candidate;
                }
            }
            unreachable!()
        }
    }
}

fn focus_and_select_stem(input: &HtmlInputElement) {
    let _ = input.focus();
    let val = input.value();
    let stem_end = val.rfind('.').unwrap_or(val.len()) as u32;
    let _ = input.set_selection_range(0, stem_end);
}

#[component]
pub fn EditorDir(
    controller: EditorDirController,
    kind: EditorDirKind,
    #[prop(into)] syntax: Signal<Option<String>>,
    #[prop(into)] readonly: Signal<bool>,
    ctrl_enter: Callback<()>,
    #[prop(into)] keyboard_mode: Signal<KeyboardMode>,
    ls_interface: Option<(LSRecv, LSSend)>,
) -> impl IntoView {
    let i18n = use_i18n();
    let EditorDirController {
        dir,
        editor_ctrl,
        files_revision,
    } = controller;
    let tabs = RwSignal::new(Vec::<String>::new());
    let workspace_files = RwSignal::new(Vec::<(String, String)>::new());
    let rename_target = RwSignal::new(None::<String>);
    let rename_value = RwSignal::new(String::new());
    let creating_file = RwSignal::new(false);
    let new_filename = RwSignal::new(String::new());
    let confirm_delete = RwSignal::new(None::<String>);
    let copied = RwSignal::new(false);

    Effect::new(move || {
        let revision = files_revision.get();
        let dir_path = dir.get();
        spawn_local(async move {
            editor_ctrl.wait_sync().await;
            let mut entries = match &dir_path {
                Some(dir_path) => {
                    let dir = common::opfs::open_dir(dir_path, true).await;
                    dir.list_entries().await
                }
                None => Vec::new(),
            };
            entries.sort();
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
                let chosen = if kind == EditorDirKind::Code {
                    syntax
                        .get_untracked()
                        .and_then(|lang| select_best_code_file(&entries, &lang, None))
                        .and_then(|entry| dir_path.as_ref().map(|d| format!("{d}/{entry}")))
                } else {
                    None
                }
                .or_else(|| {
                    files
                        .iter()
                        .find(|(filename, _)| {
                            !filename.ends_with("/grader.cpp") && !filename.ends_with("/grader.c")
                        })
                        .or_else(|| files.first())
                        .map(|(filename, _)| filename.clone())
                });
                editor_ctrl.filename.set(chosen);
            }
            tabs.set(entries);
            workspace_files.set(files);
        });
    });

    let active_basename = Signal::derive(move || {
        editor_ctrl
            .filename
            .get()
            .and_then(|f| f.split('/').next_back().map(ToOwned::to_owned))
    });

    let arm_or_remove_file = move |file: String| {
        if confirm_delete.get_untracked().as_deref() != Some(&file) {
            let target = file.clone();
            confirm_delete.set(Some(file));
            spawn_local(async move {
                TimeoutFuture::new(3000).await;
                if confirm_delete.get_untracked().as_deref() == Some(&target) {
                    confirm_delete.set(None);
                }
            });
            return;
        }
        confirm_delete.set(None);
        let Some(dir) = dir.get_untracked() else {
            tracing::error!("Directory not set when trying to remove file");
            return;
        };
        tabs.update(|t| t.retain(|f| f != &file));
        let file_path = format!("{dir}/{file}");
        editor_ctrl.filename.update(|f| {
            if f.as_deref() == Some(&file_path) {
                *f = None;
            }
        });
        spawn_local(async move {
            editor_ctrl.wait_sync().await;
            common::opfs::remove_entry(&file_path, false).await;
            files_revision.update(|revision| *revision += 1);
        });
    };

    let commit_rename = move || {
        let Some(old_name) = rename_target.get_untracked() else {
            return;
        };
        let new_name = rename_value.get_untracked().trim().to_owned();
        let Some(dir) = dir.get_untracked() else {
            rename_target.set(None);
            tracing::error!("Directory not set when trying to rename file");
            return;
        };
        if new_name == old_name {
            rename_target.set(None);
            return;
        }
        if !valid_filename(&new_name)
            || tabs
                .get_untracked()
                .iter()
                .any(|file| file != &old_name && file == &new_name)
        {
            rename_target.set(None);
            return;
        }

        rename_target.set(None);
        spawn_local(async move {
            let old_path = format!("{dir}/{old_name}");
            let new_path = format!("{dir}/{new_name}");
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
                files.sort();
            });
            controller.editor_ctrl.filename.update(|open| {
                if open.as_deref() == Some(&old_path) {
                    *open = Some(new_path);
                }
            });
            controller.files_revision.update(|revision| *revision += 1);
        });
    };

    let commit_create = move || {
        if !creating_file.get_untracked() {
            return;
        }
        let full_name = new_filename.get_untracked().trim().to_owned();
        creating_file.set(false);
        let Some(dir) = controller.dir.get_untracked() else {
            return;
        };
        if !valid_filename(&full_name) || tabs.get_untracked().iter().any(|f| f == &full_name) {
            return;
        }
        let file = format!("{dir}/{full_name}");
        spawn_local(async move {
            common::opfs::open_file(&file, true).await.write(&[]).await;
            controller.editor_ctrl.filename.set(Some(file));
            tabs.update(|t| {
                t.push(full_name);
                t.sort();
            });
            controller.files_revision.update(|revision| *revision += 1);
        });
    };

    let start_create = move |_| {
        if readonly.get_untracked() || controller.dir.get_untracked().is_none() {
            return;
        }
        rename_target.set(None);
        let suggested = suggest_filename(
            kind,
            syntax.get_untracked().as_deref(),
            &tabs.get_untracked(),
        );
        new_filename.set(suggested);
        creating_file.set(true);
    };

    let start_rename_active = move |_| {
        if readonly.get_untracked() {
            return;
        }
        if let Some(active) = active_basename.get_untracked() {
            creating_file.set(false);
            rename_value.set(active.clone());
            rename_target.set(Some(active));
        }
    };

    let render_tab = move |file: String| {
        let file_for_path = file.clone();
        let file_path = Signal::derive(move || {
            dir.get()
                .map(|d| format!("{d}/{file_for_path}"))
                .unwrap_or_default()
        });
        let file_for_active = file.clone();
        let is_active = Signal::derive(move || {
            editor_ctrl.filename.read().as_deref() == Some(&file_path.get())
                || active_basename.get().as_deref() == Some(&file_for_active)
        });
        let file_for_renaming = file.clone();
        let is_renaming =
            Signal::derive(move || rename_target.get().as_deref() == Some(&file_for_renaming));
        let file_for_bad = file.clone();
        let bad_filename = Signal::derive(move || {
            if rename_target.get().as_deref() != Some(&file_for_bad) {
                return false;
            }
            let value = rename_value.get();
            let value = value.trim();
            !valid_filename(value)
                || tabs
                    .get()
                    .iter()
                    .any(|other| other != &file_for_bad && other == value)
        });

        let file_label = file.clone();
        let file_dbl = file.clone();
        let file_del = file.clone();
        let file_confirm = file.clone();

        view! {
            <Show
                when=move || is_renaming.get()
                fallback=move || {
                    let label = file_label.clone();
                    let dbl_name = file_dbl.clone();
                    let del_name = file_del.clone();
                    let confirm_name = file_confirm.clone();
                    let is_confirming_del = Signal::derive(move || {
                        confirm_delete.get().as_deref() == Some(&confirm_name)
                    });
                    let rename_input_ref = NodeRef::<leptos::html::Input>::new();
                    let _ = rename_input_ref;
                    view! {
                        <div
                            class=move || {
                                if is_active.get() {
                                    "group flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium bg-base-100 text-base-content border-t-2 border-t-primary border-r border-base-300 cursor-pointer select-none shrink-0"
                                } else {
                                    "group flex items-center gap-1.5 px-3 py-1.5 text-xs text-base-content/70 hover:text-base-content hover:bg-base-100/50 border-t-2 border-t-transparent border-r border-base-300 cursor-pointer select-none shrink-0 transition-colors"
                                }
                            }
                            on:click=move |_| {
                                confirm_delete.set(None);
                                editor_ctrl.filename.set(Some(file_path.get_untracked()));
                            }
                            on:dblclick=move |_| {
                                if !readonly.get_untracked() {
                                    creating_file.set(false);
                                    rename_value.set(dbl_name.clone());
                                    rename_target.set(Some(dbl_name.clone()));
                                }
                            }
                        >
                            <span class="font-mono">{label}</span>
                            <Show when=move || !readonly.get()>
                                <button
                                    type="button"
                                    class=move || {
                                        if is_confirming_del.get() {
                                            "btn btn-error btn-xs h-4 min-h-0 px-1 rounded text-[10px]"
                                        } else {
                                            "opacity-40 group-hover:opacity-100 hover:text-error transition-opacity p-0.5 rounded"
                                        }
                                    }
                                    title=move || {
                                        if is_confirming_del.get() {
                                            t_string!(i18n, confirm_delete).to_string()
                                        } else {
                                            t_string!(i18n, delete_file).to_string()
                                        }
                                    }
                                    on:click={
                                        let del_name = del_name.clone();
                                        move |ev| {
                                            ev.stop_propagation();
                                            arm_or_remove_file(del_name.clone());
                                        }
                                    }
                                >
                                    <Icon
                                        attr:class="w-3 h-3"
                                        icon=Signal::derive(move || {
                                            if is_confirming_del.get() {
                                                icondata::BiTrashSolid
                                            } else {
                                                icondata::IoClose
                                            }
                                        })
                                    />
                                </button>
                            </Show>
                        </div>
                    }
                }
            >
                {
                    let input_ref = NodeRef::<leptos::html::Input>::new();
                    input_ref.on_load(move |el: HtmlInputElement| {
                        queue_microtask(move || focus_and_select_stem(&el));
                    });
                    view! {
                        <div class="flex items-center gap-1 px-1.5 py-1 bg-base-100 border-t-2 border-t-primary border-r border-base-300 shrink-0">
                            <input
                                node_ref=input_ref
                                class=move || {
                                    if bad_filename.get() {
                                        "input input-xs input-bordered input-error w-32 font-mono"
                                    } else {
                                        "input input-xs input-bordered w-32 font-mono"
                                    }
                                }
                                type="text"
                                prop:value=move || rename_value.get()
                                on:input:target=move |ev| rename_value.set(ev.target().value())
                                on:blur=move |_| commit_rename()
                                on:keydown=move |ev: KeyboardEvent| {
                                    match ev.key().as_str() {
                                        "Enter" => {
                                            ev.prevent_default();
                                            commit_rename();
                                        }
                                        "Escape" => {
                                            ev.prevent_default();
                                            rename_target.set(None);
                                        }
                                        _ => {}
                                    }
                                }
                            />
                        </div>
                    }
                }
            </Show>
        }
    };

    let import_files = move |files: FileList| {
        let Some(dir_path) = dir.get_untracked() else {
            tracing::error!("Directory not set when trying to import files");
            return;
        };
        spawn_local(async move {
            controller.wait_sync().await;
            let mut file_to_open = None;
            let mut overwritten_active = None;
            for i in 0..files.length() {
                let file = files.get(i).expect("Failed to get file from FileList");
                let file_name = file.name();
                if !valid_filename(&file_name) {
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
                tabs.update(|t| {
                    if !t.iter().any(|existing| existing == &file_name) {
                        t.push(file_name.clone());
                        t.sort();
                    }
                });
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    if controller.editor_ctrl.filename.get_untracked().as_deref()
                        == Some(&file_path)
                    {
                        overwritten_active = Some(text.to_owned());
                    }
                    if file_to_open.is_none() {
                        file_to_open = Some(file_path);
                    }
                }
            }

            if let Some(text) = overwritten_active {
                controller.set_text(&text);
            } else if let Some(file_path) = file_to_open {
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
        if readonly.get_untracked() {
            return;
        }
        let Some(data_transfer) = ev.data_transfer() else {
            return;
        };
        let Some(files) = data_transfer.files() else {
            return;
        };
        import_files(files);
    };

    let do_copy = move |_| {
        if editor_ctrl.filename.get_untracked().is_none() {
            return;
        }
        let text = controller.get_text();
        copy_to_clipboard(&text);
        copied.set(true);
        spawn_local(async move {
            TimeoutFuture::new(1500).await;
            copied.set(false);
        });
    };

    let do_download = move |_| {
        let name = editor_ctrl.filename.read();
        let Some(name) = name.as_ref() else {
            info!("no file open, download cancelled");
            return;
        };
        let text = controller.get_text();
        let name = name.split('/').next_back().unwrap_or(name);
        download(name, text.as_bytes());
    };

    let upload_el = NodeRef::<leptos::html::Input>::new();
    let do_upload = move |_| {
        let Some(input) = upload_el.get() else {
            return;
        };
        if let Some(files) = input.files() {
            import_files(files);
        }
        input.set_value("");
    };

    let bad_new_filename = Signal::derive(move || {
        let val = new_filename.get();
        let val = val.trim();
        !valid_filename(val) || tabs.get().iter().any(|f| f == val)
    });

    view! {
        <div
            class="flex flex-col h-full w-full min-h-0 bg-base-100"
            on:dragover=dragover_handler
            on:drop=drop_handler
        >
            <div class="flex items-center justify-between bg-base-200 border-b border-base-300 min-h-9 px-1 gap-1 shrink-0">
                <div class="flex items-center overflow-x-auto no-scrollbar min-w-0 flex-1">
                    <For
                        each=move || tabs.get().into_iter()
                        key=|file| file.clone()
                        children=render_tab
                    />
                    <Show when=move || creating_file.get()>
                        {
                            let create_input_ref = NodeRef::<leptos::html::Input>::new();
                            create_input_ref.on_load(move |el: HtmlInputElement| {
                                queue_microtask(move || focus_and_select_stem(&el));
                            });
                            view! {
                                <div class="flex items-center gap-1 px-1.5 py-1 bg-base-100 border-t-2 border-t-primary border-r border-base-300 shrink-0">
                                    <input
                                        node_ref=create_input_ref
                                        class=move || {
                                            if bad_new_filename.get() {
                                                "input input-xs input-bordered input-error w-32 font-mono"
                                            } else {
                                                "input input-xs input-bordered w-32 font-mono"
                                            }
                                        }
                                        type="text"
                                        prop:value=move || new_filename.get()
                                        on:input:target=move |ev| new_filename.set(ev.target().value())
                                        on:blur=move |_| commit_create()
                                        on:keydown=move |ev: KeyboardEvent| {
                                            match ev.key().as_str() {
                                                "Enter" => {
                                                    ev.prevent_default();
                                                    commit_create();
                                                }
                                                "Escape" => {
                                                    ev.prevent_default();
                                                    creating_file.set(false);
                                                }
                                                _ => {}
                                            }
                                        }
                                    />
                                </div>
                            }
                        }
                    </Show>
                    <Show when=move || {
                        controller.dir.get().is_some() && !creating_file.get() && !readonly.get()
                    }>
                        <button
                            type="button"
                            class="btn btn-ghost btn-xs btn-square mx-1 shrink-0 text-base-content/70 hover:text-base-content"
                            title=move || t_string!(i18n, new_file)
                            on:click=start_create
                        >
                            <Icon attr:class="w-3.5 h-3.5" icon=icondata::AiPlusOutlined />
                        </button>
                    </Show>
                </div>

                <div class="flex items-center gap-0.5 shrink-0 pr-1">
                    <button
                        type="button"
                        class="btn btn-ghost btn-xs btn-square"
                        title=move || {
                            if copied.get() {
                                t_string!(i18n, copied).to_string()
                            } else {
                                t_string!(i18n, copy_to_clipboard).to_string()
                            }
                        }
                        disabled=move || editor_ctrl.filename.get().is_none()
                        on:click=do_copy
                    >
                        <Icon
                            attr:class=move || {
                                if copied.get() { "w-3.5 h-3.5 text-success" } else { "w-3.5 h-3.5" }
                            }
                            icon=Signal::derive(move || {
                                if copied.get() { icondata::BiCheckRegular } else { icondata::LuCopy }
                            })
                        />
                    </button>
                    <button
                        type="button"
                        class="btn btn-ghost btn-xs btn-square"
                        title=move || t_string!(i18n, rename_file)
                        disabled=move || readonly.get() || editor_ctrl.filename.get().is_none()
                        on:click=start_rename_active
                    >
                        <Icon attr:class="w-3.5 h-3.5" icon=icondata::BiEditAltRegular />
                    </button>
                    <input
                        type="file"
                        multiple
                        class="hidden"
                        node_ref=upload_el
                        on:change=do_upload
                    />
                    <button
                        type="button"
                        class="btn btn-ghost btn-xs btn-square"
                        title=move || t_string!(i18n, upload_file)
                        disabled=move || readonly.get() || controller.dir.get().is_none()
                        on:click=move |_| {
                            if !readonly.get()
                                && let Some(el) = upload_el.get()
                            {
                                el.click();
                            }
                        }
                    >
                        <Icon attr:class="w-3.5 h-3.5" icon=icondata::ChUpload />
                    </button>
                    <button
                        type="button"
                        class="btn btn-ghost btn-xs btn-square"
                        title=move || t_string!(i18n, download_file)
                        disabled=move || editor_ctrl.filename.get().is_none()
                        on:click=do_download
                    >
                        <Icon attr:class="w-3.5 h-3.5" icon=icondata::ChDownload />
                    </button>
                </div>
            </div>
            <div class="flex-1 min-h-0 relative">
                <div
                    class="h-full w-full"
                    class:hidden=move || {
                        dir.get().is_none() || editor_ctrl.filename.get().is_none()
                    }
                >
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
                <Show when=move || dir.get().is_none() || editor_ctrl.filename.get().is_none()>
                    <div class="absolute inset-0 z-10 flex items-center justify-center bg-base-100 text-sm text-base-content/50 select-none">
                        {move || {
                            if dir.get().is_none() {
                                t_string!(i18n, no_workspace_selected)
                            } else {
                                t_string!(i18n, no_file_open)
                            }
                        }}
                    </div>
                </Show>
            </div>
        </div>
    }
}

fn valid_filename(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\'])
}
