use common::config::Config;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen_futures::spawn_local;
use web_sys::SubmitEvent;

use crate::util::Icon;
use crate::{backend, contest_api, i18n::*};

#[derive(Clone, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    pub task: Option<String>,
    pub language: String,
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

#[component]
pub fn WorkspaceSelector(
    active: RwSignal<Option<String>>,
    #[prop(into)] readonly: Signal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    let workspaces = RwSignal::new(Vec::new());
    let open = RwSignal::new(true);
    let new_name = RwSignal::new(String::new());
    let create_error = RwSignal::new(None::<CreateWorkspaceError>);
    let task = RwSignal::new(String::new());
    let language = RwSignal::new(String::new());

    spawn_local(async move {
        let dir = common::opfs::open_dir("workspace", true).await;
        let entries = dir.list_entries().await;
        workspaces.set(entries);
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
        if workspaces.read_untracked().contains(&name) {
            create_error.set(Some(CreateWorkspaceError::NameTaken));
            return;
        }
        create_error.set(None);
        let config = expect_context::<Config>();
        spawn_local(async move {
            let task = task.get_untracked();
            let language = language.get_untracked();
            let ws = if task.is_empty() {
                config.default_ws
            } else {
                contest_api::get()
                    .unwrap()
                    .init_workspace(&task, &language)
                    .await
                    .expect("Failed to initialize workspace")
            };

            for (filename, content) in ws.code {
                let code =
                    common::opfs::open_file(&format!("workspace/{name}/code/{filename}"), true)
                        .await;
                code.write(content.as_bytes()).await;
            }
            for (filename, content) in ws.stdin {
                let stdin =
                    common::opfs::open_file(&format!("workspace/{name}/stdin/{filename}"), true)
                        .await;
                stdin.write(content.as_bytes()).await;
            }
            let ws_config = serde_json::to_vec(&WorkspaceConfig {
                task: (!task.is_empty()).then_some(task),
                language,
            })
            .unwrap();
            let config_file =
                common::opfs::open_file(&format!("workspace/{name}/config.json"), true).await;
            config_file.write(&ws_config).await;

            workspaces.update(|w| w.push(name.clone()));
            active.set(Some(name));
            open.set(false);
            new_name.set(String::new());
        });
    };

    view! {
        <button class:button on:click=move |_| open.set(true) disabled=readonly>
            {move || active.get().unwrap_or_else(|| t_string!(i18n, choose_workspace).into())}
        </button>

        <div class:modal class:is-active=open>
            <div class="modal-background" on:click=move |_| open.set(false) />
            <div class="modal-card" style:max-width="48rem" style:width="calc(100vw - 2rem)">
                <header class="modal-card-head">
                    <p class="modal-card-title">{t!(i18n, workspaces)}</p>
                    <button class="delete" aria-label="close" on:click=move |_| open.set(false) />
                </header>
                <section class="modal-card-body">
                    <div class:is-flex class:is-flex-direction-column class:is-row-gap-5>
                        <div class:is-flex class:is-flex-direction-column class:is-row-gap-2>
                            <For each=move || workspaces.get() key=|w| w.clone() let:ws>
                                <WorkspaceEntry ws active open workspaces />
                            </For>
                        </div>
                        <CreateWorkspaceForm new_name create_error task language new_workspace />
                    </div>
                </section>
            </div>
        </div>
    }
}

#[component]
fn WorkspaceEntry(
    ws: String,
    active: RwSignal<Option<String>>,
    open: RwSignal<bool>,
    workspaces: RwSignal<Vec<String>>,
) -> impl IntoView {
    let ws2 = ws.clone();
    let ws3 = ws.clone();
    let select_workspace = move |_| {
        active.set(Some(ws2.clone()));
        open.set(false);
    };
    let remove_workspace = move |_| {
        workspaces.update(|w| w.retain(|x| x != &ws3));
        active.update(|a| {
            if a.as_ref() == Some(&ws3) {
                *a = None;
            }
        });
        let ws3 = ws3.clone();
        spawn_local(async move {
            let dir = common::opfs::open_dir("workspace", true).await;
            dir.remove_entry(&ws3, true).await;
        });
    };

    view! {
        <div class:is-flex class:is-justify-content-space-between class:is-column-gap-3>
            <button
                class="button is-fullwidth"
                style:justify-content="flex-start"
                on:click=select_workspace
            >
                {ws}
            </button>
            <button class="button" on:click=remove_workspace>
                <Icon icon=icondata::BiTrashSolid style:height="1em" style:width="1em" />
            </button>
        </div>
    }
}

#[component]
fn CreateWorkspaceForm(
    new_name: RwSignal<String>,
    create_error: RwSignal<Option<CreateWorkspaceError>>,
    task: RwSignal<String>,
    language: RwSignal<String>,
    new_workspace: impl Fn(SubmitEvent) + 'static + Clone,
) -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <div
            class="box"
            style:background="var(--bulma-scheme-main-bis)"
            style:border="1px solid var(--bulma-border)"
            style:box-shadow="none"
        >
            <form on:submit=new_workspace>
                <div class:field class:is-horizontal>
                    <div class:field-label class:is-normal>
                        <label class="label" style:white-space="nowrap">
                            {t!(i18n, name)}
                        </label>
                    </div>
                    <div class="field-body">
                        <div class="field">
                            <div class="control">
                                <input
                                    class="input"
                                    class:is-danger=move || create_error.get().is_some()
                                    type="text"
                                    placeholder=move || t_string!(i18n, workspace_name)
                                    prop:value=move || new_name.get()
                                    on:input:target=move |ev| {
                                        new_name.set(ev.target().value());
                                        create_error.set(None);
                                    }
                                />
                            </div>
                            <Show when=move || create_error.get().is_some()>
                                <p class:help class:is-danger>
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
                    </div>
                </div>
                <ConnectTask task />
                <Language language />
                <div class:field class:is-horizontal>
                    <div class:field-label />
                    <div class="field-body">
                        <div class="field" style:width="100%">
                            <div class="control">
                                <button class="button is-primary" class:is-fullwidth type="submit">
                                    {t!(i18n, create_workspace)}
                                </button>
                            </div>
                        </div>
                    </div>
                </div>
            </form>
        </div>
    }
}

#[component]
pub fn ConnectTask(task: RwSignal<String>) -> Option<impl IntoView> {
    let i18n = use_i18n();
    let api = contest_api::get()?;

    let tasks = LocalResource::new(move || {
        let api = api.clone();
        async move { api.list_tasks().await.unwrap() }
    });

    Some(view! {
        <div class:field class:is-horizontal>
            <div class:field-label class:is-normal>
                <label class="label">{t!(i18n, connect_to_task)}</label>
            </div>
            <div class="field-body">
                <div class="field">
                    <div class="control">
                        <div class="select is-fullwidth">
                            <select
                                prop:value=move || task.get()
                                on:change:target=move |ev| task.set(ev.target().value())
                            >
                                <option value="">{t!(i18n, none)}</option>
                                <For
                                    each=move || tasks.get().into_iter().flatten()
                                    key=|t| t.id.clone()
                                    let:task
                                >
                                    <option value=task.id>{task.name}</option>
                                </For>
                            </select>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    })
}

#[component]
pub fn Language(language: RwSignal<String>) -> Option<impl IntoView> {
    let i18n = use_i18n();
    let _api = contest_api::get()?;

    Some(view! {
        <div class:field class:is-horizontal>
            <div class:field-label class:is-normal>
                <label class="label">{t!(i18n, programming_language)}</label>
            </div>
            <div class="field-body">
                <div class="field">
                    <div class="control">
                        <div class="select is-fullwidth">
                            <select
                                prop:value=move || language.get()
                                on:change:target=move |ev| language.set(ev.target().value())
                            >
                                <For
                                    each=move || backend::languages()
                                    key=|l| l.name.clone()
                                    let:lang
                                >
                                    <option>{lang.name}</option>
                                </For>
                            </select>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    })
}
