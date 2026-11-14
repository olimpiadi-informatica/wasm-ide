use std::collections::HashMap;

use futures_util::future::join_all;
use gloo_timers::future::TimeoutFuture;
use gloo_utils::window;
use leptos::ev::keydown;
use leptos::prelude::*;
use leptos_use::{on_click_outside, use_document, use_event_listener};
use serde::{Deserialize, Serialize};
use tracing::warn;
use wasm_bindgen_futures::spawn_local;
use web_sys::SubmitEvent;

use crate::config::{Config, Workspace};
use crate::i18n::*;
use crate::settings::use_settings;
use crate::util::Icon;
use crate::{backend, contest_api};

#[derive(Clone, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    pub task: Option<String>,
    pub language: String,
    #[serde(default)]
    pub automatic: bool,
}

impl WorkspaceConfig {
    pub async fn load(name: &str) -> anyhow::Result<Self> {
        let config_file =
            common::opfs::open_file(&format!("workspace/{name}/config.json"), false).await;
        let config = config_file.read().await;
        Ok(serde_json::from_slice(&config)?)
    }
}

pub const DEFAULT_WORKSPACE: &str = "default";

pub fn primary_extension_for_lang(lang: &str) -> String {
    backend::languages()
        .into_iter()
        .find(|l| l.name == lang)
        .and_then(|l| l.extensions.first().cloned())
        .unwrap_or_else(|| "cpp".to_owned())
}

async fn initialize_workspace(name: &str, workspace: &Workspace, config: &WorkspaceConfig) {
    for (filename, content) in &workspace.code {
        let code =
            common::opfs::open_file(&format!("workspace/{name}/code/{filename}"), true).await;
        code.write(content.as_bytes()).await;
    }
    for (filename, content) in &workspace.stdin {
        let stdin =
            common::opfs::open_file(&format!("workspace/{name}/stdin/{filename}"), true).await;
        stdin.write(content.as_bytes()).await;
    }
    let config = serde_json::to_vec(config).unwrap();
    let config_file = common::opfs::open_file(&format!("workspace/{name}/config.json"), true).await;
    config_file.write(&config).await;
}

pub async fn ensure_default_workspace(default_workspace: &Workspace, language: &str) {
    let workspace_dir = common::opfs::open_dir("workspace", true).await;
    if workspace_dir.contains_dir(DEFAULT_WORKSPACE).await {
        return;
    }

    let config = WorkspaceConfig {
        task: None,
        language: language.to_owned(),
        automatic: false,
    };
    initialize_workspace(DEFAULT_WORKSPACE, default_workspace, &config).await;
}

const ACTIVE_WORKSPACE_KEY: &str = "wasm_ide_active_workspace";

pub fn get_saved_workspace() -> Option<String> {
    window()
        .local_storage()
        .expect("failed to get localStorage")?
        .get_item(ACTIVE_WORKSPACE_KEY)
        .expect("failed to read active workspace")
}

pub fn set_saved_workspace(ws: Option<&str>) {
    let Some(storage) = window()
        .local_storage()
        .expect("failed to get localStorage")
    else {
        return;
    };
    match ws {
        Some(ws) => storage.set_item(ACTIVE_WORKSPACE_KEY, ws),
        None => storage.remove_item(ACTIVE_WORKSPACE_KEY),
    }
    .expect("failed to update active workspace");
}

pub async fn ensure_contest_workspaces(
    api: &contest_api::DynContestAPI,
    language: &str,
) -> anyhow::Result<()> {
    let workspace_dir = common::opfs::open_dir("workspace", true).await;
    let tasks = api.list_tasks().await?;
    let futures = tasks.iter().map(|task| async {
        let task_id = &task.id;
        if workspace_dir.contains_dir(task_id).await {
            return Ok(());
        }
        let res = async {
            let ws = api.init_workspace(task_id, language).await?;
            let config = WorkspaceConfig {
                task: Some(task_id.clone()),
                language: language.to_string(),
                automatic: true,
            };
            initialize_workspace(task_id, &ws, &config).await;
            Ok::<_, anyhow::Error>(())
        }
        .await;

        if let Err(err) = &res {
            if workspace_dir.contains_dir(task_id).await {
                workspace_dir.remove_entry(task_id, true).await;
            }
            warn!("Failed to initialize workspace for task '{task_id}': {err:?}");
        }
        res
    });
    let results = join_all(futures).await;
    results.into_iter().collect::<anyhow::Result<Vec<()>>>()?;
    Ok(())
}

#[derive(Clone, Copy)]
enum CreateWorkspaceError {
    EmptyName,
    InvalidName,
    NameTaken,
}

fn valid_workspace_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\'])
}

fn suggest_workspace_name(base: &str, existing: &[WorkspaceItem]) -> String {
    if !existing.iter().any(|w| w.name == base) {
        return base.to_owned();
    }
    for i in 2.. {
        let candidate = format!("{base}-{i}");
        if !existing.iter().any(|w| w.name == candidate) {
            return candidate;
        }
    }
    unreachable!()
}

#[derive(Clone, PartialEq)]
struct WorkspaceItem {
    name: String,
    task: Option<String>,
    task_title: Option<String>,
    language: String,
    automatic: bool,
    score: Option<(f64, f64)>,
}

#[component]
pub fn WorkspaceSelector(
    active: RwSignal<Option<String>>,
    #[prop(into)] active_language: Signal<String>,
    #[prop(into)] readonly: Signal<bool>,
    #[prop(into)] workspaces_revision: Signal<u64>,
    #[prop(into)] contest_ready: Signal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    let settings = use_settings();
    let app_config = StoredValue::new(expect_context::<Config>());
    let resolve_default_lang = move || {
        settings
            .default_language
            .get_untracked()
            .or_else(|| backend::languages().first().map(|l| l.name.clone()))
            .unwrap_or_else(|| "C++".to_owned())
    };

    let workspaces = RwSignal::new(Vec::<WorkspaceItem>::new());
    let open = RwSignal::new(false);
    let show_create_form = RwSignal::new(false);
    let new_name = RwSignal::new(String::new());
    let name_customized = RwSignal::new(false);
    let create_error = RwSignal::new(None::<CreateWorkspaceError>);
    let task = RwSignal::new(String::new());
    let language = RwSignal::new(resolve_default_lang());
    let confirm_delete = RwSignal::new(None::<String>);

    let refresh_workspaces = move || {
        spawn_local(async move {
            let dir = common::opfs::open_dir("workspace", true).await;
            let mut entries = dir.list_entries().await;
            entries.sort();

            let api = contest_api::get();
            let task_titles: HashMap<String, String> = if let Some(api) = &api {
                api.list_tasks()
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|t| (t.id, t.name))
                    .collect()
            } else {
                HashMap::new()
            };

            let mut items = Vec::with_capacity(entries.len());
            for name in entries {
                let cfg = WorkspaceConfig::load(&name).await.ok();
                let automatic = cfg.as_ref().map(|c| c.automatic).unwrap_or(false);
                let task_id = cfg.as_ref().and_then(|c| c.task.clone());
                let task_title = task_id.as_ref().and_then(|id| task_titles.get(id).cloned());
                let mut lang = cfg.map(|c| c.language).unwrap_or_default();
                if lang.is_empty() {
                    let code_dir =
                        common::opfs::open_dir(&format!("workspace/{name}/code"), true).await;
                    let code_entries = code_dir.list_entries().await;
                    let langs = backend::languages();
                    lang = code_entries
                        .iter()
                        .find_map(|entry| {
                            let ext = entry.rsplit('.').next().unwrap_or("");
                            langs
                                .iter()
                                .find(|l| l.extensions.iter().any(|e| e == ext))
                                .map(|l| l.name.clone())
                        })
                        .unwrap_or_else(|| "C++".to_owned());
                }
                items.push(WorkspaceItem {
                    name,
                    task: task_id,
                    task_title,
                    language: lang,
                    automatic,
                    score: None,
                });
            }
            if items.is_empty() {
                show_create_form.set(true);
            }
            workspaces.set(items.clone());

            if let Some(api) = api {
                for item in items {
                    if let Some(task_id) = item.task {
                        let api = api.clone();
                        let ws_name = item.name;
                        spawn_local(async move {
                            if let Ok(score) = api.task_score(&task_id).await {
                                workspaces.update(|list| {
                                    if let Some(w) = list.iter_mut().find(|w| w.name == ws_name) {
                                        w.score = Some(score);
                                    }
                                });
                            }
                        });
                    }
                }
            }
        });
    };

    Effect::new(move |_| {
        if active.get().is_none() && contest_ready.get() {
            open.set(true);
            show_create_form.set(true);
        }
    });

    Effect::new(move |_| {
        workspaces_revision.get();
        refresh_workspaces();
    });

    Effect::new(move |_| {
        if open.get() {
            confirm_delete.set(None);
            refresh_workspaces();
        }
    });

    Effect::new(move |_| {
        if show_create_form.get() {
            language.set(resolve_default_lang());
            if !name_customized.get_untracked() && new_name.get_untracked().trim().is_empty() {
                let selected_task = task.get_untracked();
                let base = if selected_task.is_empty() {
                    "workspace"
                } else {
                    &selected_task
                };
                new_name.set(suggest_workspace_name(base, &workspaces.get_untracked()));
            }
        }
    });

    let dropdown_ref = NodeRef::<leptos::html::Div>::new();
    let _ = on_click_outside(dropdown_ref, move |_| open.set(false));
    let _ = use_event_listener(use_document(), keydown, move |evt| {
        if evt.key_code() == 27 {
            open.set(false);
        }
    });

    let new_workspace = move |ev: SubmitEvent| {
        ev.prevent_default();
        let name = new_name.get_untracked().trim().to_string();
        if name.is_empty() {
            create_error.set(Some(CreateWorkspaceError::EmptyName));
            return;
        }
        if !valid_workspace_name(&name) {
            create_error.set(Some(CreateWorkspaceError::InvalidName));
            return;
        }
        if workspaces.read_untracked().iter().any(|w| w.name == name) {
            create_error.set(Some(CreateWorkspaceError::NameTaken));
            return;
        }
        create_error.set(None);
        let config = app_config.get_value();
        spawn_local(async move {
            let selected_task = task.get_untracked();
            let mut selected_lang = language.get_untracked();
            if selected_lang.is_empty() {
                selected_lang = resolve_default_lang();
            }
            let ws = if selected_task.is_empty() {
                let ext = primary_extension_for_lang(&selected_lang);
                config.local_workspace(&selected_lang, &ext)
            } else {
                match contest_api::get()
                    .unwrap()
                    .init_workspace(&selected_task, &selected_lang)
                    .await
                {
                    Ok(ws) => ws,
                    Err(err) => {
                        warn!("Failed to initialize workspace for task '{selected_task}': {err:?}");
                        return;
                    }
                }
            };

            let ws_config = WorkspaceConfig {
                task: (!selected_task.is_empty()).then(|| selected_task.clone()),
                language: selected_lang.clone(),
                automatic: false,
            };
            initialize_workspace(&name, &ws, &ws_config).await;

            workspaces.update(|w| {
                w.push(WorkspaceItem {
                    name: name.clone(),
                    task: (!selected_task.is_empty()).then_some(selected_task),
                    task_title: None,
                    language: selected_lang,
                    automatic: false,
                    score: None,
                });
                w.sort_by(|a, b| a.name.cmp(&b.name));
            });
            active.set(Some(name));
            open.set(false);
            show_create_form.set(false);
            new_name.set(String::new());
            name_customized.set(false);
        });
    };

    view! {
        <div class="relative" node_ref=dropdown_ref>
            <button
                type="button"
                class="btn btn-sm btn-outline border-base-300 bg-base-100 gap-2 font-medium"
                disabled=readonly
                on:click=move |_| open.update(|v| *v = !*v)
            >
                <Icon attr:class="w-4 h-4 text-primary" icon=icondata::BiFolderOpenSolid />
                <span class="font-mono text-xs sm:text-sm max-w-32 sm:max-w-44 truncate">
                    {move || active.get().unwrap_or_else(|| t_string!(i18n, choose_workspace).into())}
                </span>
                <Show when=move || active.get().is_some()>
                    <span class="badge badge-ghost badge-xs font-mono">
                        {move || active_language.get()}
                    </span>
                </Show>
                <Icon attr:class="w-3.5 h-3.5 opacity-60" icon=icondata::BiChevronDownRegular />
            </button>

            <Show when=move || open.get()>
                <div class="absolute left-0 top-full mt-1.5 w-[26rem] max-w-[calc(100vw-1.5rem)] bg-base-100 border border-base-300 rounded-box shadow-xl z-[2000] p-3 flex flex-col gap-3">
                    <div class="flex items-center justify-between border-b border-base-300 pb-2">
                        <span class="font-semibold text-sm flex items-center gap-1.5">
                            <Icon attr:class="w-4 h-4 text-primary" icon=icondata::BiFolderOpenSolid />
                            {t!(i18n, workspaces)}
                        </span>
                        <button
                            type="button"
                            class=move || {
                                if show_create_form.get() {
                                    "btn btn-xs btn-ghost gap-1"
                                } else {
                                    "btn btn-xs btn-primary gap-1"
                                }
                            }
                            on:click=move |_| {
                                show_create_form.update(|v| *v = !*v);
                                create_error.set(None);
                            }
                        >
                            <Icon
                                attr:class="w-3.5 h-3.5"
                                icon=Signal::derive(move || {
                                    if show_create_form.get() {
                                        icondata::IoClose
                                    } else {
                                        icondata::AiPlusOutlined
                                    }
                                })
                            />
                            {move || {
                                if show_create_form.get() {
                                    t_string!(i18n, cancel).to_string()
                                } else {
                                    t_string!(i18n, new_workspace).to_string()
                                }
                            }}
                        </button>
                    </div>

                    <Show when=move || !workspaces.get().is_empty()>
                        <div class="flex flex-col gap-1 max-h-64 overflow-y-auto pr-0.5">
                            <For each=move || workspaces.get() key=|w| w.name.clone() let:item>
                                <WorkspaceEntry
                                    item
                                    active
                                    open
                                    workspaces
                                    confirm_delete
                                />
                            </For>
                        </div>
                    </Show>

                    <Show when=move || show_create_form.get() || workspaces.get().is_empty()>
                        <CreateWorkspaceForm
                            new_name
                            name_customized
                            create_error
                            task
                            language
                            workspaces
                            new_workspace
                        />
                    </Show>
                </div>
            </Show>
        </div>
    }
}

#[component]
fn WorkspaceEntry(
    item: WorkspaceItem,
    active: RwSignal<Option<String>>,
    open: RwSignal<bool>,
    workspaces: RwSignal<Vec<WorkspaceItem>>,
    confirm_delete: RwSignal<Option<String>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let name = item.name.clone();
    let select_name = name.clone();
    let active_check_name = name.clone();
    let is_active = Signal::derive(move || active.get().as_deref() == Some(&active_check_name));

    let select_workspace = move |_| {
        active.set(Some(select_name.clone()));
        open.set(false);
    };

    let del_check_name = name.clone();
    let is_confirming_del =
        Signal::derive(move || confirm_delete.get().as_deref() == Some(&del_check_name));

    let remove_name = name.clone();
    let remove_workspace = move |ev: web_sys::MouseEvent| {
        ev.stop_propagation();
        if confirm_delete.get_untracked().as_deref() != Some(&remove_name) {
            let target = remove_name.clone();
            confirm_delete.set(Some(remove_name.clone()));
            spawn_local(async move {
                TimeoutFuture::new(3000).await;
                if confirm_delete.get_untracked().as_deref() == Some(&target) {
                    confirm_delete.set(None);
                }
            });
            return;
        }
        confirm_delete.set(None);
        workspaces.update(|w| w.retain(|x| x.name != remove_name));
        let next_ws = workspaces.with_untracked(|w| w.first().map(|x| x.name.clone()));
        active.update(|a| {
            if a.as_ref() == Some(&remove_name) {
                *a = next_ws;
            }
        });
        let remove_name = remove_name.clone();
        spawn_local(async move {
            let dir = common::opfs::open_dir("workspace", true).await;
            dir.remove_entry(&remove_name, true).await;
        });
    };

    let subtitle = item
        .task_title
        .clone()
        .filter(|t| t != &item.name)
        .or_else(|| item.task.clone().filter(|t| t != &item.name));
    let lang_badge = (!item.language.is_empty()).then_some(item.language.clone());
    let score_info = item.score;

    view! {
        <div
            class=move || {
                if is_active.get() {
                    "flex items-center justify-between gap-2 px-2.5 py-2 rounded-lg bg-primary/10 border border-primary/30 cursor-pointer transition-colors"
                } else {
                    "flex items-center justify-between gap-2 px-2.5 py-2 rounded-lg hover:bg-base-200 border border-transparent cursor-pointer transition-colors"
                }
            }
            on:click=select_workspace
        >
            <div class="flex flex-col min-w-0 flex-1">
                <div class="flex items-center gap-2 min-w-0">
                    <span class=move || {
                        if is_active.get() {
                            "font-mono text-sm font-semibold text-primary truncate"
                        } else {
                            "font-mono text-sm font-medium truncate"
                        }
                    }>{name}</span>
                    {lang_badge.map(|lang| {
                        view! { <span class="badge badge-ghost badge-xs font-mono shrink-0">{lang}</span> }
                    })}
                </div>
                {subtitle.map(|sub| {
                    view! { <span class="text-xs text-base-content/60 truncate">{sub}</span> }
                })}
            </div>

            <div class="flex items-center gap-1 shrink-0">
                {score_info.map(|(score, max_score)| {
                    let badge_cls = if max_score > 0.0 && score >= max_score {
                        "badge badge-success badge-sm font-mono"
                    } else if score > 0.0 {
                        "badge badge-warning badge-sm font-mono"
                    } else {
                        "badge badge-ghost badge-sm font-mono"
                    };
                    view! {
                        <span class=badge_cls>{format!("{score:.0}/{max_score:.0}")}</span>
                    }
                })}

                {(!item.automatic).then(|| {
                    view! {
                        <button
                            type="button"
                            class=move || {
                                if is_confirming_del.get() {
                                    "btn btn-error btn-xs btn-square"
                                } else {
                                    "btn btn-ghost btn-xs btn-square text-base-content/60 hover:text-error"
                                }
                            }
                            title=move || {
                                if is_confirming_del.get() {
                                    t_string!(i18n, confirm_delete).to_string()
                                } else {
                                    t_string!(i18n, delete_workspace).to_string()
                                }
                            }
                            on:click=remove_workspace
                        >
                            <Icon attr:class="w-3.5 h-3.5" icon=icondata::BiTrashSolid />
                        </button>
                    }
                })}
            </div>
        </div>
    }
}

#[component]
fn CreateWorkspaceForm(
    new_name: RwSignal<String>,
    name_customized: RwSignal<bool>,
    create_error: RwSignal<Option<CreateWorkspaceError>>,
    task: RwSignal<String>,
    language: RwSignal<String>,
    workspaces: RwSignal<Vec<WorkspaceItem>>,
    new_workspace: impl Fn(SubmitEvent) + 'static + Clone,
) -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <form
            class="bg-base-200/70 border border-base-300 rounded-lg p-3 flex flex-col gap-2.5"
            on:submit=new_workspace
        >
            <ConnectTask task new_name name_customized workspaces />
            <Language language />

            <div class="flex flex-col gap-1">
                <label class="text-xs font-medium text-base-content/80">{t!(i18n, name)}</label>
                <div class="flex items-center gap-2">
                    <input
                        class=move || {
                            if create_error.get().is_some() {
                                "input input-bordered input-sm input-error flex-1 font-mono"
                            } else {
                                "input input-bordered input-sm flex-1 font-mono"
                            }
                        }
                        type="text"
                        placeholder=move || t_string!(i18n, workspace_name)
                        prop:value=move || new_name.get()
                        on:input:target=move |ev| {
                            let val = ev.target().value();
                            name_customized.set(!val.trim().is_empty());
                            new_name.set(val);
                            create_error.set(None);
                        }
                    />
                    <button class="btn btn-primary btn-sm shrink-0" type="submit">
                        {t!(i18n, create_workspace)}
                    </button>
                </div>
                <Show when=move || create_error.get().is_some()>
                    <p class="text-xs text-error">
                        {move || match create_error.get() {
                            Some(CreateWorkspaceError::EmptyName) => {
                                t_string!(i18n, workspace_name_empty).to_string()
                            }
                            Some(CreateWorkspaceError::InvalidName) => {
                                t_string!(i18n, workspace_name_invalid).to_string()
                            }
                            Some(CreateWorkspaceError::NameTaken) => {
                                t_string!(i18n, workspace_name_taken).to_string()
                            }
                            None => String::new(),
                        }}
                    </p>
                </Show>
            </div>
        </form>
    }
}

#[component]
fn ConnectTask(
    task: RwSignal<String>,
    new_name: RwSignal<String>,
    name_customized: RwSignal<bool>,
    workspaces: RwSignal<Vec<WorkspaceItem>>,
) -> Option<impl IntoView> {
    let i18n = use_i18n();
    let api = contest_api::get()?;

    let tasks = LocalResource::new(move || {
        let api = api.clone();
        async move { api.list_tasks().await.unwrap_or_default() }
    });

    Some(view! {
        <div class="flex flex-col gap-1">
            <label class="text-xs font-medium text-base-content/80">
                {t!(i18n, connect_to_task)}
            </label>
            <select
                class="select select-bordered select-sm w-full"
                prop:value=move || task.get()
                on:change:target=move |ev| {
                    let selected = ev.target().value();
                    task.set(selected.clone());
                    if !name_customized.get_untracked()
                        || new_name.get_untracked().trim().is_empty()
                    {
                        let base = if selected.is_empty() {
                            "workspace"
                        } else {
                            &selected
                        };
                        let suggested =
                            suggest_workspace_name(base, &workspaces.get_untracked());
                        new_name.set(suggested);
                    }
                }
            >
                <option value="">{t!(i18n, none)}</option>
                <For
                    each=move || tasks.get().into_iter().flatten()
                    key=|t| t.id.clone()
                    let:task_item
                >
                    <option value=task_item.id>{task_item.name}</option>
                </For>
            </select>
        </div>
    })
}

#[component]
fn Language(language: RwSignal<String>) -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <div class="flex flex-col gap-1">
            <label class="text-xs font-medium text-base-content/80">
                {t!(i18n, programming_language)}
            </label>
            <select
                class="select select-bordered select-sm w-full"
                prop:value=move || language.get()
                on:change:target=move |ev| language.set(ev.target().value())
            >
                <For each=move || backend::languages() key=|l| l.name.clone() let:lang>
                    <option>{lang.name}</option>
                </For>
            </select>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::{DEFAULT_WORKSPACE, WorkspaceConfig, ensure_default_workspace};
    use crate::config::Workspace;

    async fn clear_workspaces() {
        let root = common::opfs::root().await;
        if root.contains_dir("workspace").await {
            root.remove_entry("workspace", true).await;
        }
    }

    #[wasm_bindgen_test]
    async fn default_workspace_is_created_without_overwriting_existing_files() {
        clear_workspaces().await;

        let workspace = Workspace {
            code: [("main.cpp".to_owned(), "initial source".to_owned().into())]
                .into_iter()
                .collect(),
            stdin: [("input.txt".to_owned(), "initial input".to_owned().into())]
                .into_iter()
                .collect(),
        };
        ensure_default_workspace(&workspace, "C++").await;

        let code_path = format!("workspace/{DEFAULT_WORKSPACE}/code/main.cpp");
        let input_path = format!("workspace/{DEFAULT_WORKSPACE}/stdin/input.txt");
        assert_eq!(
            common::opfs::open_file(&code_path, false)
                .await
                .read()
                .await,
            b"initial source"
        );
        assert_eq!(
            common::opfs::open_file(&input_path, false)
                .await
                .read()
                .await,
            b"initial input"
        );

        let config =
            common::opfs::open_file(&format!("workspace/{DEFAULT_WORKSPACE}/config.json"), false)
                .await
                .read()
                .await;
        let config: WorkspaceConfig = serde_json::from_slice(&config).unwrap();
        assert!(config.task.is_none());
        assert_eq!(config.language, "C++");

        common::opfs::open_file(&code_path, false)
            .await
            .write(b"user source")
            .await;
        common::opfs::open_file(&input_path, false)
            .await
            .write(b"user input")
            .await;

        let replacement = Workspace {
            code: [(
                "replacement.cpp".to_owned(),
                "replacement".to_owned().into(),
            )]
            .into_iter()
            .collect(),
            stdin: [(
                "replacement.txt".to_owned(),
                "replacement".to_owned().into(),
            )]
            .into_iter()
            .collect(),
        };
        ensure_default_workspace(&replacement, "Python3").await;

        assert_eq!(
            common::opfs::open_file(&code_path, false)
                .await
                .read()
                .await,
            b"user source"
        );
        assert_eq!(
            common::opfs::open_file(&input_path, false)
                .await
                .read()
                .await,
            b"user input"
        );
        let default =
            common::opfs::open_dir(&format!("workspace/{DEFAULT_WORKSPACE}"), false).await;
        assert!(
            !default
                .open_dir("code", false)
                .await
                .list_entries()
                .await
                .contains(&"replacement.cpp".to_owned())
        );
        assert!(
            !default
                .open_dir("stdin", false)
                .await
                .list_entries()
                .await
                .contains(&"replacement.txt".to_owned())
        );

        clear_workspaces().await;
    }
}
