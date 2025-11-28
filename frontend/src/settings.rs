use std::num::IntErrorKind;

use common::Language;
use leptos::ev::keydown;
use leptos::prelude::*;
use leptos::reactive::wrappers::write::SignalSetter;
use leptos::server::codee::string::JsonSerdeCodec;
use leptos_use::storage::use_local_storage;
use leptos_use::{on_click_outside, use_document, use_event_listener, use_preferred_dark};
use serde::{Deserialize, Serialize};
use strum::VariantArray;
use tracing::info;

use crate::enum_select::{DisplayLocalized, EnumSelect};
use crate::i18n::*;
use crate::util::Icon;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

#[derive(PartialEq, Eq, Clone, Copy, Hash, Debug, Serialize, Deserialize, VariantArray)]
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
struct StoredSettings {
    theme: Option<Theme>,
    keyboard_mode: KeyboardMode,
    input_mode: InputMode,
    editor_width_percent: f32,
    language: Language,
    mem_limit: Option<u32>,
}

impl Default for StoredSettings {
    fn default() -> Self {
        Self {
            theme: None,
            keyboard_mode: KeyboardMode::Standard,
            input_mode: InputMode::Batch,
            editor_width_percent: 65.0,
            language: Language::CPP,
            mem_limit: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SettingsProvider {
    write: WriteSignal<StoredSettings>,
    read: Signal<StoredSettings>,
    pub theme: Signal<Theme>,
    pub editor_width_percent: Signal<f32>,
    pub keyboard_mode: Signal<KeyboardMode>,
    pub input_mode: Signal<InputMode>,
    pub language: Signal<Language>,
    pub mem_limit: Signal<Option<u32>>,
}

impl SettingsProvider {
    pub fn install() {
        let (read_settings, write_settings, _) =
            use_local_storage::<StoredSettings, JsonSerdeCodec>("wasm_ide_settings");
        let prefers_dark = use_preferred_dark();
        let theme = Memo::new(move |_| {
            read_settings.get().theme.unwrap_or(if prefers_dark.get() {
                Theme::Dark
            } else {
                Theme::Light
            })
        });
        // Sync the theme to the entire page
        Effect::new(move || {
            info!(theme = ?theme.get());
            document()
                .document_element()
                .unwrap()
                .class_list()
                .set_value(if theme.get() == Theme::Dark {
                    "theme-dark"
                } else {
                    "theme-light"
                });
        });
        provide_context(Self {
            write: write_settings,
            read: read_settings,
            theme: theme.into(),
            editor_width_percent: Memo::new(move |_| read_settings.get().editor_width_percent)
                .into(),
            keyboard_mode: Memo::new(move |_| read_settings.get().keyboard_mode).into(),
            input_mode: Memo::new(move |_| read_settings.get().input_mode).into(),
            language: Memo::new(move |_| read_settings.get().language).into(),
            mem_limit: Memo::new(move |_| read_settings.get().mem_limit).into(),
        });
    }
}

pub fn use_settings() -> SettingsProvider {
    expect_context()
}

pub const MIN_EDITOR_WIDTH: f32 = 35.0;
pub const MAX_EDITOR_WIDTH: f32 = 75.0;

pub fn set_editor_width(val: f32) {
    use_settings().write.update(|v| {
        v.editor_width_percent = val.clamp(MIN_EDITOR_WIDTH, MAX_EDITOR_WIDTH);
    });
}

pub fn set_language(language: Language) {
    use_settings().write.update(|v| v.language = language);
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
        <Icon
            class:is-size-3
            class:mx-2
            class:is-clickable
            icon=icondata::LuSettings
            on:click=move |_| open.set(true)
        />
        <div class:modal class:is-active=move || open.get() style:--bulma-modal-z="10000" style:--bulma-modal-content-width="50rem">
            <div class="modal-background" />
            <div class="modal-content" node_ref=content>
                <div class="box">
                    <div class:field class:is-horizontal>
                        <div class:field-label class:is-normal>
                            <label class="label">{t!(i18n, language)}</label>
                        </div>
                        <div class="field-body">
                            <div class="control">
                                <EnumSelect value=locale_value />
                            </div>
                        </div>
                    </div>
                    <div class:field class:is-horizontal>
                        <div class:field-label class:is-normal>
                            <label class="label">{t!(i18n, keyboard_mode)}</label>
                        </div>
                        <div class="field-body">
                            <div class="control">
                                <EnumSelect value=(keyboard_mode, SignalSetter::map(set_kb_mode)) />
                            </div>
                        </div>
                    </div>
                    <ThemeControl />
                    <hr />
                    <MemLimit />
                </div>
            </div>
        </div>
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

    const WIDTH: &str = "12em";

    view! {
        <div class:field class:is-horizontal>
            <div class:field-label class:is-normal>
                <label class="label">{t!(i18n, theme)}</label>
            </div>
            <div class="field-body">
                <div class="control">
                    <div class:buttons class:has-addons>
                        <button
                            class:has-icons-left
                            class:button
                            class:is-info=move || theme.get() == Some(Theme::Dark)
                            style:width=WIDTH
                            on:click=move |_| set_theme(Some(Theme::Dark))
                        >
                            <Icon class:icon class:is-left class:mr-1 icon=icondata::BiMoonSolid />
                            {t!(i18n, theme_dark)}
                        </button>
                        <button
                            class:has-icons-left
                            class:button
                            class:is-info=move || theme.get() == Some(Theme::Light)
                            style:width=WIDTH
                            on:click=move |_| set_theme(Some(Theme::Light))
                        >
                            <Icon class:icon class:is-left class:mr-1 icon=icondata::BiSunSolid />
                            {t!(i18n, theme_light)}
                        </button>
                        <button
                            class:has-icons-left
                            class:button
                            class:is-info=move || theme.get().is_none()
                            style:width=WIDTH
                            on:click=move |_| set_theme(None)
                        >
                            <Icon class:icon class:is-left class:mr-1 icon=system_theme />
                            {t!(i18n, theme_system)}
                        </button>
                    </div>
                </div>
            </div>
        </div>
    }
}

#[component]
fn MemLimit() -> impl IntoView {
    let settings = use_settings();

    #[derive(Debug, Clone, Copy)]
    enum Error {
        NotANumber,
        TooSmall,
        TooLarge,
    }

    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Error::NotANumber => write!(f, "Please enter a valid number"),
                Error::TooSmall => write!(f, "Value must be at least 40"),
                Error::TooLarge => write!(f, "Value must be at most 4096"),
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
        <div class:field class:is-horizontal>
            <div class:field-label class:is-normal>
                <label class="label">{"Memory limit"}</label>
            </div>
            <div class="field-body">
                <div class="field has-addons">
                    <div class="control">
                        <input
                            class:input
                            class:is-danger=move || error.get().is_some()
                            on:input=on_input
                            type="text"
                            node_ref=input_ref
                            value=settings
                                .mem_limit
                                .get_untracked()
                                .map_or("".to_string(), |v| v.to_string())
                        />
                        <ShowLet some=error let:value>
                            <p class:help class:is-danger>
                                {value.to_string()}
                            </p>
                        </ShowLet>
                    </div>
                    <div class="control">
                        <a class="button is-static">MiB</a>
                    </div>
                </div>
            </div>
        </div>
    }
}
