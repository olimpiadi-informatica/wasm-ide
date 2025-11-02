use std::ops::{Deref, DerefMut};

use common::WorkerExecStatus;
use leptos::either::{EitherOf4, EitherOf5};
use leptos::prelude::*;
use tracing::warn;

use crate::i18n::*;
use crate::{FetchingCompilerProgress, RunState, StateExec, StateLS};

#[derive(Default)]
#[slot]
struct MessageHeader {
    #[prop(optional)]
    children: Option<Children>,
}

#[component]
fn Message(
    #[prop(into)] kind: Signal<String>,
    #[prop(optional)] message_header: MessageHeader,
    children: Children,
) -> impl IntoView {
    view! {
        <div
            style:position="fixed"
            style:top="0.5em"
            style:left="50%"
            style:transform="translateX(-50%)"
            style:z-index="1000"
            style:box-shadow="0 0 1em 0 hsla(var(--bulma-shadow-h), var(--bulma-shadow-s), var(--bulma-shadow-l), .1)"
            class=move || format!("message {}", kind.get())
        >
            {message_header.children.map(|h| view! { <div class="message-header">{h()}</div> })}
            <div class:message-body>{children()}</div>
        </div>
    }
}

#[component]
pub fn StatusView(
    state: RwSignal<RunState>,
    fetching_compiler_progress: RwSignal<FetchingCompilerProgress>,
) -> impl IntoView {
    let i18n = use_i18n();

    let render_exec = move |exec: &StateExec| {
        match exec {
            StateExec::Ready | StateExec::Complete { error: None, .. } => None,

            StateExec::Processing { stopping: true, .. } => Some(EitherOf5::A(
                view! { <Message kind="is-warning">{t!(i18n, stopping_execution)}</Message> },
            )),

            // TODO(virv): status: None should have its own variant?
            StateExec::Processing { status: None, .. }
            | StateExec::Processing {
                status: Some(WorkerExecStatus::FetchingCompiler),
                ..
            } => Some(EitherOf5::B(
                view! { <FetchingCompilerMessageBar fetching_compiler_progress /> },
            )),

            StateExec::Processing {
                status: Some(WorkerExecStatus::Compiling),
                ..
            } => Some(EitherOf5::C(
                view! { <Message kind="is-success">{t!(i18n, compiling)}</Message> },
            )),

            StateExec::Processing {
                status: Some(WorkerExecStatus::Running),
                ..
            } => Some(EitherOf5::D(
                view! { <Message kind="is-success">{t!(i18n, executing)}</Message> },
            )),

            StateExec::Complete {
                error: Some(err), ..
            } => Some(EitherOf5::E(view! {
                <ErrorMessageBar
                    err
                    clear=move || {
                        match state.write().deref_mut() {
                            RunState::Ready { exec: StateExec::Complete { error, .. }, .. } => {
                                *error = None;
                            }
                            _ => warn!("Unexpected state when hiding error"),
                        }
                    }
                />
            })),
        }
    };

    let render_ls = move |ls: &StateLS| match ls {
        StateLS::Ready => None,
        StateLS::Requested => None,
        StateLS::FetchingCompiler => {
            Some(view! { <FetchingCompilerMessageBar fetching_compiler_progress /> }.into_any())
        }
        StateLS::Running => None,
        StateLS::Error(err) => Some(
            view! {
                <ErrorMessageBar
                    err
                    clear=move || {
                        match state.write().deref_mut() {
                            RunState::Ready { ls: ls @ StateLS::Error(_), .. } => {
                                *ls = StateLS::Ready;
                            }
                            _ => warn!("Unexpected state when hiding LS error"),
                        }
                    }
                />
            }
            .into_any(),
        ),
    };

    move || match state.read().deref() {
        RunState::Loading => {
            EitherOf4::A(view! { <Message kind="is-info">{t!(i18n, loading)}</Message> })
        }

        RunState::Ready { exec, ls } => {
            if let Some(view) = render_exec(exec) {
                EitherOf4::B(view)
            } else if let Some(view) = render_ls(ls) {
                EitherOf4::C(view)
            } else {
                EitherOf4::D(())
            }
        }
    }
}

#[component]
fn ErrorMessageBar(
    #[prop(into)] err: String,
    clear: impl Fn() + Send + Sync + 'static + Clone,
) -> impl IntoView {
    let i18n = use_i18n();
    let title = t_string!(i18n, hide_error);
    let clear = move |_| clear();
    view! {
        <Message kind="is-danger">
            <MessageHeader slot>
                <p>{t!(i18n, error)}</p>
                <button
                    class="delete"
                    aria-label="delete"
                    title=title
                    on:click=clear.clone()
                ></button>
            </MessageHeader>
            <pre>{err}</pre>
        </Message>
    }
}

#[component]
fn FetchingCompilerMessageBar(
    fetching_compiler_progress: RwSignal<FetchingCompilerProgress>,
) -> impl IntoView {
    let i18n = use_i18n();

    let render_progress = move |name: String| {
        let progress = {
            let name = name.clone();
            Signal::derive(move || {
                fetching_compiler_progress
                    .read()
                    .get(&name)
                    .flatten()
                    .cloned()
            })
        };
        view! {
            <tr>
                <td class:is-family-monospace>{name}</td>
                <td style:vertical-align="middle">
                    <progress
                        class:progress
                        class:is-primary
                        style:margin-bottom="0"
                        style:width="20em"
                        value=move || progress.get().map(|x| x.0)
                        max=move || progress.get().map(|x| x.1)
                    />
                </td>
                <td style:width="4em" style:text-align="right">
                    {move || {
                        progress
                            .get()
                            .map(|(cur, tot)| { format!("{:.1}%", 100. * cur as f64 / tot as f64) })
                    }}
                </td>
            </tr>
        }
    };

    view! {
        <Message kind="is-primary">
            <h3>{t!(i18n, downloading_runtime)}</h3>
            <table
                class:table
                style:--bulma-table-background-color="none"
                style:--bulma-table-color="inherit"
                style:--bulma-table-cell-border-width="0"
            >
                <tbody>
                    <For
                        each=move || fetching_compiler_progress.get()
                        key=|x| x.0.clone()
                        let((name, _))
                    >
                        {move || render_progress(name.clone())}
                    </For>
                </tbody>
            </table>
        </Message>
    }
}
