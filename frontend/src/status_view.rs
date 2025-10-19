use std::ops::{Deref, DerefMut};

use common::WorkerExecStatus;
use leptos::{either::EitherOf4, prelude::*};
use thaw::{
    Button, MessageBar, MessageBarActions, MessageBarBody, MessageBarIntent, MessageBarLayout,
    MessageBarTitle,
};
use tracing::warn;

use crate::{i18n::*, FetchingCompilerProgress, RunState, StateExec, StateLS};

#[component]
pub fn StatusView(
    state: RwSignal<RunState>,
    fetching_compiler_progress: RwSignal<FetchingCompilerProgress>,
) -> impl IntoView {
    let i18n = use_i18n();

    let render_exec = move |exec: &StateExec| {
        match exec {
            StateExec::Ready => None,

            StateExec::Processing { stopping: true, .. } => Some(
                view! {
                    <MessageBar class="status-view" intent=MessageBarIntent::Warning>
                        <MessageBarBody>{t!(i18n, stopping_execution)}</MessageBarBody>
                    </MessageBar>
                }
                .into_any(),
            ),

            // TODO(virv): status: None should have its own variant?
            StateExec::Processing { status: None, .. }
            | StateExec::Processing {
                status: Some(WorkerExecStatus::FetchingCompiler),
                ..
            } => {
                Some(view! { <FetchingCompilerMessageBar fetching_compiler_progress /> }.into_any())
            }

            StateExec::Processing {
                status: Some(WorkerExecStatus::Compiling),
                ..
            } => Some(
                view! {
                    <MessageBar class="status-view" intent=MessageBarIntent::Success>
                        <MessageBarBody>{t!(i18n, compiling)}</MessageBarBody>
                    </MessageBar>
                }
                .into_any(),
            ),

            StateExec::Processing {
                status: Some(WorkerExecStatus::Running),
                ..
            } => Some(
                view! {
                    <MessageBar class="status-view" intent=MessageBarIntent::Success>
                        <MessageBarBody>{t!(i18n, executing)}</MessageBarBody>
                    </MessageBar>
                }
                .into_any(),
            ),

            StateExec::Complete {
                error: Some(err), ..
            } => Some(
                view! {
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
                }
                .into_any(),
            ),

            StateExec::Complete { error: None, .. } => None,
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
        RunState::Loading => EitherOf4::A(view! {
            <MessageBar class="status-view" intent=MessageBarIntent::Success>
                <MessageBarBody>{t!(i18n, loading)}</MessageBarBody>
            </MessageBar>
        }),

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
    clear: impl Fn() + Send + Sync + 'static,
) -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <MessageBar
            class="status-view"
            intent=MessageBarIntent::Error
            layout=MessageBarLayout::Multiline
        >
            <MessageBarBody>
                <MessageBarTitle>{t!(i18n, error)}</MessageBarTitle>
                <pre>{err}</pre>
            </MessageBarBody>
            <MessageBarActions>
                <Button
                    class="red"
                    icon=icondata::AiCloseOutlined
                    on_click=move |_| clear()
                    block=true
                >
                    {t!(i18n, hide_error)}
                </Button>
            </MessageBarActions>
        </MessageBar>
    }
}

#[component]
fn FetchingCompilerMessageBar(
    fetching_compiler_progress: RwSignal<FetchingCompilerProgress>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <MessageBar
            class="status-view"
            intent=MessageBarIntent::Success
            layout=MessageBarLayout::Multiline
        >
            <MessageBarBody>
                <MessageBarTitle>{t!(i18n, downloading_runtime)}</MessageBarTitle>
                <For
                    each=move || fetching_compiler_progress.get()
                    key=|x| x.clone()
                    let((name, progress))
                >
                    <div style="display: flex; flex-direction: row; align-items: center; gap: 20px;">
                        <pre>{name}</pre>
                        <progress value=progress.map(|x| x.0) max=progress.map(|x| x.1) />
                        <span style="width: 3em; text-align: right;">
                            {progress
                                .map(|(cur, tot)| {
                                    format!("{:.1}%", 100. * cur as f64 / tot as f64)
                                })}
                        </span>
                    </div>
                </For>
            </MessageBarBody>
        </MessageBar>
    }
}
