use std::num::IntErrorKind;

use leptos::ev::keydown;
use leptos::prelude::*;
use leptos::reactive::wrappers::write::SignalSetter;
use leptos::server::codee::string::JsonSerdeCodec;
use leptos_use::storage::{UseStorageOptions, use_local_storage_with_options};
use leptos_use::{on_click_outside, use_document, use_event_listener, use_preferred_dark};
use serde::{Deserialize, Serialize};
use strum::VariantArray;
use tracing::info;

use crate::enum_select::{DisplayLocalized, EnumSelect};
use crate::i18n::*;
use crate::util::Icon;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    Light,
    Dark,
}

#[derive(PartialEq, Eq, Clone, Copy, Hash, Debug, Serialize, Deserialize, VariantArray)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardMode {
    Standard,
    Vim,
    Emacs,
}

impl DisplayLocalized for KeyboardMode {
    fn to_localized_string(&self, locale: Locale) -> String {
        match self {
            KeyboardMode::Vim => td_display!(locale, vim_mode),
            KeyboardMode::Emacs => td_display!(locale, emacs_mode),
            KeyboardMode::Standard => td_display!(locale, standard_mode),
        }
        .to_string()
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Hash, Debug, Serialize, Deserialize, VariantArray)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    Batch,
    MixedInteractive,
    FullInteractive,
}

impl DisplayLocalized for InputMode {
    fn to_localized_string(&self, locale: Locale) -> String {
        match self {
            InputMode::Batch => td_display!(locale, batch_input),
            InputMode::MixedInteractive => td_display!(locale, mixed_interactive_input),
            InputMode::FullInteractive => td_display!(locale, full_interactive_input),
        }
        .to_string()
    }
}

const fn default_input_height_percent() -> f32 {
    50.0
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub(crate) struct StoredSettings {
    theme: Option<Theme>,
    keyboard_mode: KeyboardMode,
    input_mode: InputMode,
    persist_storage: bool,
    editor_width_percent: f32,
    #[serde(default = "default_input_height_percent")]
    input_height_percent: f32,
    mem_limit: Option<u32>,
    time_limit: Option<f64>,
}

impl Default for StoredSettings {
    fn default() -> Self {
        Self {
            theme: None,
            keyboard_mode: KeyboardMode::Standard,
            input_mode: InputMode::Batch,
            persist_storage: false,
            editor_width_percent: 65.0,
            input_height_percent: default_input_height_percent(),
            mem_limit: None,
            time_limit: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SettingsProvider {
    write: WriteSignal<StoredSettings>,
    read: Signal<StoredSettings>,
    pub theme: Signal<Theme>,
    pub editor_width_percent: Signal<f32>,
    pub input_height_percent: Signal<f32>,
    pub keyboard_mode: Signal<KeyboardMode>,
    pub input_mode: Signal<InputMode>,
    pub persist_storage: Signal<bool>,
    pub mem_limit: Signal<Option<u32>>,
    pub time_limit: Signal<Option<f64>>,
}

impl SettingsProvider {
    pub fn install(default_settings: StoredSettings) {
        let (read, write, _) = use_local_storage_with_options::<StoredSettings, JsonSerdeCodec>(
            "wasm_ide_settings",
            UseStorageOptions::default().initial_value(default_settings),
        );
        let prefers_dark = use_preferred_dark();
        let theme = Memo::new(move |_| {
            read.get().theme.unwrap_or(if prefers_dark.get() {
                Theme::Dark
            } else {
                Theme::Light
            })
        });
        // Sync the theme to the entire page using DaisyUI data-theme
        Effect::new(move || {
            info!(theme = ?theme.get());
            if let Some(root) = document().document_element() {
                let daisy_theme = match theme.get() {
                    Theme::Light => "winter",
                    Theme::Dark => "night",
                };
                let _ = root.set_attribute("data-theme", daisy_theme);
            }
        });
        provide_context(Self {
            write,
            read,
            theme: theme.into(),
            editor_width_percent: Memo::new(move |_| read.get().editor_width_percent).into(),
            input_height_percent: Memo::new(move |_| read.get().input_height_percent).into(),
            keyboard_mode: Memo::new(move |_| read.get().keyboard_mode).into(),
            input_mode: Memo::new(move |_| read.get().input_mode).into(),
            persist_storage: Memo::new(move |_| read.get().persist_storage).into(),
            mem_limit: Memo::new(move |_| read.get().mem_limit).into(),
            time_limit: Memo::new(move |_| read.get().time_limit).into(),
        });
    }
}

pub fn use_settings() -> SettingsProvider {
    expect_context()
}

pub const MIN_EDITOR_WIDTH: f32 = 30.0;
pub const MAX_EDITOR_WIDTH: f32 = 80.0;

pub fn set_editor_width(val: f32) {
    use_settings().write.update(|v| {
        v.editor_width_percent = val.clamp(MIN_EDITOR_WIDTH, MAX_EDITOR_WIDTH);
    });
}

pub const MIN_INPUT_HEIGHT: f32 = 20.0;
pub const MAX_INPUT_HEIGHT: f32 = 80.0;

pub fn set_input_height(val: f32) {
    use_settings().write.update(|v| {
        v.input_height_percent = val.clamp(MIN_INPUT_HEIGHT, MAX_INPUT_HEIGHT);
    });
}

pub fn set_input_mode(input_mode: InputMode) {
    use_settings().write.update(|v| v.input_mode = input_mode);
}

#[component]
pub fn Settings() -> impl IntoView {
    let i18n = use_i18n();
    let open = RwSignal::new(false);

    let SettingsProvider { keyboard_mode, .. } = use_settings();

    let set_kb_mode = move |kb_mode| {
        expect_context::<SettingsProvider>()
            .write
            .update(|v| v.keyboard_mode = kb_mode);
    };

    let locale_value = (
        Signal::derive(move || i18n.get_locale()),
        SignalSetter::map(move |new_locale: Locale| {
            i18n.set_locale(new_locale);
        }),
    );

    let content = NodeRef::new();

    let _ = on_click_outside(content, move |_| open.set(false));
    let _ = use_event_listener(use_document(), keydown, move |evt| {
        if evt.key_code() == 27 {
            open.set(false);
        }
    });

    view! {
        <button
            type="button"
            class="btn btn-ghost btn-sm btn-square"
            title=move || t_string!(i18n, settings)
            on:click=move |_| open.set(true)
        >
            <Icon attr:class="w-5 h-5" icon=icondata::LuSettings />
        </button>
        <Show when=move || open.get()>
            <dialog class="modal modal-open z-[10000]">
                <div class="modal-box max-w-2xl" node_ref=content>
                    <div class="flex items-center justify-between pb-3 border-b border-base-300 mb-4">
                        <h3 class="font-bold text-lg flex items-center gap-2">
                            <Icon attr:class="w-5 h-5" icon=icondata::LuSettings />
                            {t!(i18n, settings)}
                        </h3>
                        <button
                            type="button"
                            class="btn btn-sm btn-circle btn-ghost"
                            aria-label="close"
                            on:click=move |_| open.set(false)
                        >
                            <Icon attr:class="w-4 h-4" icon=icondata::IoClose />
                        </button>
                    </div>

                    <div class="flex flex-col gap-4">
                        <div class="grid grid-cols-1 sm:grid-cols-3 items-center gap-2">
                            <label class="text-sm font-medium">{t!(i18n, interface_language)}</label>
                            <div class="sm:col-span-2">
                                <EnumSelect value=locale_value />
                            </div>
                        </div>

                        <div class="grid grid-cols-1 sm:grid-cols-3 items-center gap-2">
                            <label class="text-sm font-medium">{t!(i18n, keyboard_mode)}</label>
                            <div class="sm:col-span-2">
                                <EnumSelect value=(keyboard_mode, SignalSetter::map(set_kb_mode)) />
                            </div>
                        </div>

                        <ThemeControl />
                        <PersistStorageControl />

                        <div class="divider my-1" />

                        <TimeLimit />
                        <MemLimit />

                        <p class="text-center text-base-content/50 text-xs mt-2 font-mono">
                            {concat!("wasm-ide ", env!("WASM_IDE_VERSION"))}
                        </p>
                    </div>
                </div>
                <div class="modal-backdrop bg-black/40" on:click=move |_| open.set(false) />
            </dialog>
        </Show>
    }
}

#[component]
fn ThemeControl() -> impl IntoView {
    let SettingsProvider { read, .. } = use_settings();
    let i18n = use_i18n();
    let theme = Signal::derive(move || read.get().theme);
    let set_theme = move |theme| {
        expect_context::<SettingsProvider>()
            .write
            .update(|v| v.theme = theme);
    };

    let preferred_dark = leptos_use::use_preferred_dark();
    let system_theme = Signal::derive(move || {
        if preferred_dark.get() {
            icondata::BiMoonSolid
        } else {
            icondata::BiSunSolid
        }
    });

    view! {
        <div class="grid grid-cols-1 sm:grid-cols-3 items-center gap-2">
            <label class="text-sm font-medium">{t!(i18n, theme)}</label>
            <div class="sm:col-span-2">
                <div class="join">
                    <button
                        type="button"
                        class=move || {
                            if theme.get() == Some(Theme::Dark) {
                                "btn btn-sm join-item btn-primary gap-1.5"
                            } else {
                                "btn btn-sm join-item btn-outline border-base-300 gap-1.5"
                            }
                        }
                        on:click=move |_| set_theme(Some(Theme::Dark))
                    >
                        <Icon icon=icondata::BiMoonSolid />
                        {t!(i18n, theme_dark)}
                    </button>
                    <button
                        type="button"
                        class=move || {
                            if theme.get() == Some(Theme::Light) {
                                "btn btn-sm join-item btn-primary gap-1.5"
                            } else {
                                "btn btn-sm join-item btn-outline border-base-300 gap-1.5"
                            }
                        }
                        on:click=move |_| set_theme(Some(Theme::Light))
                    >
                        <Icon icon=icondata::BiSunSolid />
                        {t!(i18n, theme_light)}
                    </button>
                    <button
                        type="button"
                        class=move || {
                            if theme.get().is_none() {
                                "btn btn-sm join-item btn-primary gap-1.5"
                            } else {
                                "btn btn-sm join-item btn-outline border-base-300 gap-1.5"
                            }
                        }
                        on:click=move |_| set_theme(None)
                    >
                        <Icon icon=system_theme />
                        {t!(i18n, theme_system)}
                    </button>
                </div>
            </div>
        </div>
    }
}

#[component]
fn PersistStorageControl() -> impl IntoView {
    let i18n = use_i18n();
    let settings = use_settings();

    let toggle = move |_| {
        settings
            .write
            .update(|v| v.persist_storage = !v.persist_storage);
    };

    view! {
        <div class="grid grid-cols-1 sm:grid-cols-3 items-center gap-2">
            <label class="text-sm font-medium">{t!(i18n, persist_storage)}</label>
            <div class="sm:col-span-2 flex items-center gap-3">
                <input
                    type="checkbox"
                    class="toggle toggle-primary toggle-sm"
                    prop:checked=move || settings.persist_storage.get()
                    on:change=toggle
                />
                <span class="text-sm opacity-80">
                    {move || {
                        if settings.persist_storage.get() {
                            t_string!(i18n, enabled).to_string()
                        } else {
                            t_string!(i18n, disabled).to_string()
                        }
                    }}
                </span>
            </div>
        </div>
    }
}

#[component]
fn MemLimit() -> impl IntoView {
    let i18n = use_i18n();
    let settings = use_settings();

    #[derive(Debug, Clone, Copy)]
    enum Error {
        NotANumber,
        TooSmall,
        TooLarge,
    }

    impl Error {
        fn localized_message(&self, loc: Locale) -> String {
            match self {
                Error::NotANumber => td_string!(loc, please_enter_a_valid_number).into(),
                Error::TooSmall => td_string!(loc, value_must_be_at_least_40).into(),
                Error::TooLarge => td_string!(loc, value_must_be_at_most_4096).into(),
            }
        }
    }

    let error = RwSignal::new(None);
    let input_ref = NodeRef::<leptos::html::Input>::new();

    let on_input = move |_| {
        let input = input_ref.get().unwrap();
        let value = input.value();
        let value = value.trim();

        match value.parse() {
            Ok(..40) => {
                error.set(Some(Error::TooSmall));
            }
            Ok(4097..) => {
                error.set(Some(Error::TooLarge));
            }
            Ok(v) => {
                settings.write.update(|s| s.mem_limit = Some(v));
                error.set(None);
            }
            Err(e) => match e.kind() {
                IntErrorKind::Empty => {
                    settings.write.update(|s| s.mem_limit = None);
                    error.set(None);
                }
                IntErrorKind::InvalidDigit => {
                    error.set(Some(Error::NotANumber));
                }
                IntErrorKind::PosOverflow => {
                    error.set(Some(Error::TooLarge));
                }
                IntErrorKind::NegOverflow => {
                    error.set(Some(Error::TooSmall));
                }
                _ => {
                    error.set(Some(Error::NotANumber));
                }
            },
        };
    };

    view! {
        <div class="grid grid-cols-1 sm:grid-cols-3 items-start gap-2">
            <label class="text-sm font-medium pt-1.5">{t!(i18n, memory_limit)}</label>
            <div class="sm:col-span-2 flex flex-col gap-1">
                <div class="join">
                    <input
                        class=move || {
                            if error.get().is_some() {
                                "input input-bordered input-sm input-error join-item w-40 font-mono"
                            } else {
                                "input input-bordered input-sm join-item w-40 font-mono"
                            }
                        }
                        on:input=on_input
                        type="text"
                        node_ref=input_ref
                        value=settings
                            .mem_limit
                            .get_untracked()
                            .map_or("".to_string(), |v| v.to_string())
                    />
                    <span class="join-item inline-flex items-center px-3 bg-base-200 border border-l-0 border-base-300 text-xs font-mono font-medium text-base-content select-none">
                        "MiB"
                    </span>
                </div>
                <ShowLet some=error let:value>
                    <p class="text-xs text-error">
                        {move || value.localized_message(i18n.get_locale())}
                    </p>
                </ShowLet>
            </div>
        </div>
    }
}

#[component]
fn TimeLimit() -> impl IntoView {
    let i18n = use_i18n();
    let settings = use_settings();

    #[derive(Debug, Clone, Copy)]
    enum Error {
        NotANumber,
        NotPositive,
    }

    impl Error {
        fn localized_message(&self, loc: Locale) -> String {
            match self {
                Error::NotANumber => td_string!(loc, please_enter_a_valid_number).into(),
                Error::NotPositive => td_string!(loc, value_must_be_positive).into(),
            }
        }
    }

    let error = RwSignal::new(None);
    let input_ref = NodeRef::<leptos::html::Input>::new();

    let on_input = move |_| {
        let input = input_ref.get().unwrap();
        let value = input.value();
        let value = value.trim();

        match value.parse() {
            _ if value.is_empty() => {
                settings.write.update(|s| s.time_limit = None);
                error.set(None);
            }
            Ok(v) if v <= 0. => {
                error.set(Some(Error::NotPositive));
            }
            Ok(v) => {
                settings.write.update(|s| s.time_limit = Some(v));
                error.set(None);
            }
            Err(_) => {
                // The message provided by parse_float is not very user-friendly, so we handle all
                // errors the same way
                error.set(Some(Error::NotANumber));
            }
        };
    };

    view! {
        <div class="grid grid-cols-1 sm:grid-cols-3 items-start gap-2">
            <label class="text-sm font-medium pt-1.5">{t!(i18n, time_limit)}</label>
            <div class="sm:col-span-2 flex flex-col gap-1">
                <div class="join">
                    <input
                        class=move || {
                            if error.get().is_some() {
                                "input input-bordered input-sm input-error join-item w-40 font-mono"
                            } else {
                                "input input-bordered input-sm join-item w-40 font-mono"
                            }
                        }
                        on:input=on_input
                        type="text"
                        node_ref=input_ref
                        value=settings
                            .time_limit
                            .get_untracked()
                            .map_or("".to_string(), |v| v.to_string())
                    />
                    <span class="join-item inline-flex items-center px-3 bg-base-200 border border-l-0 border-base-300 text-xs font-mono font-medium text-base-content select-none">
                        "s"
                    </span>
                </div>
                <ShowLet some=error let:value>
                    <p class="text-xs text-error">
                        {move || value.localized_message(i18n.get_locale())}
                    </p>
                </ShowLet>
            </div>
        </div>
    }
}
