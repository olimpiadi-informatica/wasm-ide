use common::config::Config;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::SubmitEvent;

use crate::util::Icon;
use crate::{backend, contest_api, i18n::*};

#[component]
pub fn WorkspaceSelector(
    active: RwSignal<Option<String>>,
    #[prop(into)] readonly: Signal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    let workspaces = RwSignal::new(Vec::new());
    let open = RwSignal::new(true);
    let new_name = RwSignal::new(String::new());
    let task = RwSignal::new(String::new());
    let language = RwSignal::new(String::new());

    spawn_local(async move {
        let dir = common::opfs::open_dir("workspace", true).await;
        let entries = dir.list_entries().await;
        workspaces.set(entries);
    });

    let new_workspace = move |ev: SubmitEvent| {
        ev.prevent_default();
        let name = new_name.get_untracked();
        if name.is_empty() {
            return;
        }
        let config = expect_context::<Config>();
        spawn_local(async move {
            let task = task.get_untracked();
            let ws = if task.is_empty() {
                config.default_ws
            } else {
                contest_api::get()
                    .unwrap()
                    .init_workspace(&task, &language.get_untracked())
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

            workspaces.update(|w| w.push(name.clone()));
            active.set(Some(name));
            open.set(false);
            new_name.set(String::new());
        });
    };

    let render_ws = move |ws: String| {
        let ws2 = ws.clone();
        let ws3 = ws.clone();
        view! {
            <a on:click=move |_| {
                active.set(Some(ws2.clone()));
                open.set(false);
            }>{ws}</a>
            <Icon
                icon=icondata::BiTrashSolid
                class:is-clickable
                style:height="1em"
                style:width="1em"
                on:click=move |_| {
                    workspaces.update(|w| w.retain(|x| x != &ws3));
                    active
                        .update(|a| {
                            if a.as_ref() == Some(&ws3) {
                                *a = None;
                            }
                        });
                    let ws3 = ws3.clone();
                    spawn_local(async move {
                        let dir = common::opfs::open_dir("workspace", true).await;
                        dir.remove_entry(&ws3, true).await;
                    });
                }
            />
        }
    };

    view! {
        <button class:button on:click=move |_| open.set(true) disabled=readonly>
            {move || active.get().unwrap_or_else(|| t_string!(i18n, choose_workspace).into())}
        </button>

        <div class:modal class:is-active=open>
            <div class="modal-background" on:click=move |_| open.set(false) />
            <div class="modal-card">
                <header class="modal-card-head">
                    <p class="modal-card-title">{t!(i18n, workspaces)}</p>
                    <button class="delete" aria-label="close" on:click=move |_| open.set(false) />
                </header>
                <section
                    class="modal-card-body"
                    style:display="grid"
                    style:grid-template-columns="auto 1em"
                >
                    <form
                        style:grid-column="span 2"
                        style:display="grid"
                        style:grid-template-columns="1fr 1fr"
                        style:gap="0.5em"
                        class:mb-6
                        on:submit=new_workspace
                    >
                        <span>{t!(i18n, name)}</span>
                        <input
                            class="input"
                            type="text"
                            placeholder=move || t_string!(i18n, workspace_name)
                            bind:value=new_name
                        />
                        <ConnectTask task />
                        <Language language />
                        <button
                            class="button is-primary"
                            type="submit"
                            style:grid-column="span 2"
                            class:mx-2
                        >
                            {t!(i18n, create_workspace)}
                        </button>
                    </form>
                    <For each=move || workspaces.get() key=|w| w.clone() children=render_ws />
                </section>
            </div>
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
        <span>{t!(i18n, connect_to_task)}</span>
        <div class:select>
            <select
                prop:value=move || task.get()
                on:change:target=move |ev| task.set(ev.target().value())
                style:width="100%"
            >
                <option value="">{t!(i18n, none)}</option>
                <For each=move || tasks.get().into_iter().flatten() key=|t| t.id.clone() let:task>
                    <option value=task.id>{task.name}</option>
                </For>
            </select>
        </div>
    })
}

#[component]
pub fn Language(language: RwSignal<String>) -> Option<impl IntoView> {
    let i18n = use_i18n();
    let _api = contest_api::get()?;

    Some(view! {
        <span>{t!(i18n, programming_language)}</span>
        <div class:select>
            <select
                prop:value=move || language.get()
                on:change:target=move |ev| language.set(ev.target().value())
                style:width="100%"
            >
                <For each=move || backend::languages() key=|l| l.name.clone() let:lang>
                    <option>{lang.name}</option>
                </For>
            </select>
        </div>
    })
}
