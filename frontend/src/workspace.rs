use common::config::Config;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::SubmitEvent;

use crate::i18n::*;
use crate::util::Icon;

#[component]
pub fn WorkspaceSelector(
    active: RwSignal<Option<String>>,
    #[prop(into)] readonly: Signal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    let workspaces = RwSignal::new(Vec::new());
    let open = RwSignal::new(true);
    let new_ws = RwSignal::new(String::new());

    spawn_local(async move {
        let dir = common::opfs::open_dir("workspace", true).await;
        let entries = dir.list_entries().await;
        workspaces.set(entries);
    });

    let new_workspace = move |ev: SubmitEvent| {
        ev.prevent_default();
        let name = new_ws.get_untracked();
        if name.is_empty() {
            return;
        }
        let config = expect_context::<Config>();
        spawn_local(async move {
            for (filename, content) in config.default_ws.code {
                let code =
                    common::opfs::open_file(&format!("workspace/{name}/code/{filename}"), true)
                        .await;
                code.write(content.as_bytes()).await;
            }
            for (filename, content) in config.default_ws.stdin {
                let stdin =
                    common::opfs::open_file(&format!("workspace/{name}/stdin/{filename}"), true)
                        .await;
                stdin.write(content.as_bytes()).await;
            }

            workspaces.update(|w| w.push(name.clone()));
            active.set(Some(name));
            open.set(false);
            new_ws.set(String::new());
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
                        class:is-flex
                        class:is-column-gap-2
                        class:is-align-items-center
                        class:mb-6
                        on:submit=new_workspace
                    >
                        <input
                            class="input"
                            type="text"
                            placeholder=move || t_string!(i18n, workspace_name)
                            bind:value=new_ws
                        />
                        <button class="button is-primary" type="submit">
                            {t!(i18n, create_workspace)}
                        </button>
                    </form>
                    <For each=move || workspaces.get() key=|w| w.clone() children=render_ws />
                </section>
            </div>
        </div>
    }
}
