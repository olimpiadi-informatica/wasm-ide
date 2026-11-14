use common::WorkerExecStatus;
use leptos::prelude::*;
use tracing::warn;

use crate::config::Config;
use crate::i18n::*;
use crate::util::Icon;
use crate::{FetchingCompilerProgress, RunState, StateExec, StateLS, StateSubmit};

#[component]
pub fn StatusView(
    state: RwSignal<RunState>,
    fetching_compiler_progress: RwSignal<FetchingCompilerProgress>,
) -> impl IntoView {
    let i18n = use_i18n();

    let is_fetching_compiler = Memo::new(move |_| {
        let s = state.read();
        matches!(
            s.exec,
            StateExec::Processing {
                status: Some(WorkerExecStatus::FetchingCompiler),
                ..
            }
        ) || matches!(s.ls, StateLS::FetchingCompiler)
            || !fetching_compiler_progress.read().is_empty()
    });

    let ls_error = Memo::new(move |_| match &state.read().ls {
        StateLS::Error(err) => Some(err.clone()),
        _ => None,
    });

    let is_submitting =
        Memo::new(move |_| matches!(state.read().submit, StateSubmit::Submitting(_)));

    let submit_complete = Memo::new(move |_| match &state.read().submit {
        StateSubmit::Complete(status) => Some((status.score, status.message.clone())),
        _ => None,
    });

    let submit_error = Memo::new(move |_| match &state.read().submit {
        StateSubmit::Error(err) => Some(err.clone()),
        _ => None,
    });

    view! {
        <div class="toast toast-bottom toast-end z-[1000] gap-2 p-3 pointer-events-none">
            <Show when=move || is_fetching_compiler.get()>
                <div class="pointer-events-auto">
                    <FetchingCompilerMessageBar fetching_compiler_progress />
                </div>
            </Show>

            <Show when=move || is_submitting.get()>
                <div class="alert alert-info shadow-lg py-2.5 px-4 text-sm pointer-events-auto">
                    <span class="loading loading-spinner loading-sm" />
                    <span>{t!(i18n, submitting)}</span>
                </div>
            </Show>

            <ShowLet some=submit_complete let:((score, message))>
                <div class="alert alert-success shadow-lg py-3 px-4 text-sm flex flex-col items-stretch gap-1 max-w-md pointer-events-auto">
                    <div class="flex items-center justify-between gap-4">
                        <div class="flex items-center gap-2 font-semibold">
                            <Icon attr:class="w-4 h-4 shrink-0" icon=icondata::BiCheckCircleSolid />
                            <span>{t!(i18n, submission)}</span>
                        </div>
                        <button
                            type="button"
                            class="btn btn-ghost btn-xs btn-circle"
                            aria-label="close"
                            on:click=move |_| {
                                state.update(|s| {
                                    s.submit = StateSubmit::Ready;
                                });
                            }
                        >
                            <Icon attr:class="w-3.5 h-3.5" icon=icondata::IoClose />
                        </button>
                    </div>
                    <p class="font-mono font-medium">{t!(i18n, submission_score, score = score)}</p>
                    {message.map(|msg| {
                        view! {
                            <pre class="text-xs bg-base-100/20 p-2 rounded overflow-auto max-h-40 whitespace-pre-wrap">
                                {msg}
                            </pre>
                        }
                    })}
                </div>
            </ShowLet>

            <ShowLet some=submit_error let:err>
                <div class="pointer-events-auto">
                    <ErrorMessageBar
                        err
                        clear=move || {
                            state.update(|s| {
                                s.submit = StateSubmit::Ready;
                            });
                        }
                    />
                </div>
            </ShowLet>

            <ShowLet some=ls_error let:err>
                <div class="pointer-events-auto">
                    <ErrorMessageBar
                        err
                        clear=move || {
                            match &mut state.write().ls {
                                ls @ StateLS::Error(_) => {
                                    *ls = StateLS::Ready;
                                }
                                _ => warn!("Unexpected state when hiding LS error"),
                            }
                        }
                    />
                </div>
            </ShowLet>
        </div>
    }
}

#[component]
fn ErrorMessageBar(
    #[prop(into)] err: String,
    clear: impl Fn() + Send + Sync + 'static + Clone,
) -> impl IntoView {
    let i18n = use_i18n();
    let title = move || t_string!(i18n, hide_error);
    let clear = move |_| clear();
    view! {
        <div class="alert alert-error shadow-lg py-3 px-4 text-sm flex flex-col items-stretch gap-1 max-w-md">
            <div class="flex items-center justify-between gap-4">
                <div class="flex items-center gap-2 font-semibold">
                    <Icon attr:class="w-4 h-4 shrink-0" icon=icondata::BiErrorCircleSolid />
                    <span>{t!(i18n, error)}</span>
                </div>
                <button
                    type="button"
                    class="btn btn-ghost btn-xs btn-circle"
                    aria-label="close"
                    title=title
                    on:click=clear
                >
                    <Icon attr:class="w-3.5 h-3.5" icon=icondata::IoClose />
                </button>
            </div>
            <pre class="text-xs font-mono whitespace-pre-wrap break-all max-h-40 overflow-auto">
                {err}
            </pre>
        </div>
    }
}

#[component]
fn FetchingCompilerMessageBar(
    fetching_compiler_progress: RwSignal<FetchingCompilerProgress>,
) -> impl IntoView {
    let i18n = use_i18n();
    let app_config = StoredValue::new(expect_context::<Config>());

    let render_progress = move |name: String| {
        let progress = {
            let name = name.clone();
            Signal::derive(move || fetching_compiler_progress.read().get(&name).cloned())
        };
        let max = app_config.with_value(|cfg| cfg.worker.compilers.get(&name).copied());
        view! {
            <div class="flex items-center gap-3 text-xs">
                <span class="font-mono font-medium w-16 truncate">{name}</span>
                <progress
                    class="progress progress-primary w-40 h-2"
                    value=move || progress.get()
                    max=max
                />
                <span class="font-mono w-12 text-right">
                    {move || {
                        progress
                            .get()
                            .zip(max)
                            .map(|(cur, tot)| format!("{:.0}%", 100. * cur as f64 / tot as f64))
                    }}
                </span>
            </div>
        }
    };

    view! {
        <div class="alert bg-base-100 border border-base-300 shadow-lg py-3 px-4 text-sm flex flex-col items-stretch gap-2">
            <div class="flex items-center gap-2 font-semibold text-xs">
                <span class="loading loading-spinner loading-xs text-primary" />
                <span>{t!(i18n, downloading_runtime)}</span>
            </div>
            <div class="flex flex-col gap-1.5">
                <For
                    each=move || fetching_compiler_progress.get()
                    key=|x| x.0.clone()
                    let((name, _))
                >
                    {move || render_progress(name.clone())}
                </For>
            </div>
        </div>
    }
}
