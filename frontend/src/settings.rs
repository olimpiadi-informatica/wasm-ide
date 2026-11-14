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

use crate::backend;
use crate::enum_select::{DisplayLocalized, EnumSelect};
use crate::i18n::*;
use crate::util::Icon;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, VariantArray)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    Light,
    CatppuccinLatte,
    SolarizedLight,
    NordLight,
    Dark,
    OneDark,
    Dracula,
    CatppuccinMocha,
    Nord,
    GruvboxDark,
    SolarizedDark,
}

impl Theme {
    pub const LIGHT_PRESETS: &'static [Theme] = &[
        Theme::Light,
        Theme::CatppuccinLatte,
        Theme::SolarizedLight,
        Theme::NordLight,
    ];

    pub const DARK_PRESETS: &'static [Theme] = &[
        Theme::Dark,
        Theme::OneDark,
        Theme::Dracula,
        Theme::CatppuccinMocha,
        Theme::Nord,
        Theme::GruvboxDark,
        Theme::SolarizedDark,
    ];

    pub const fn daisy_theme(self) -> &'static str {
        match self {
            Theme::Light => "winter",
            Theme::CatppuccinLatte => "catppuccin-latte",
            Theme::SolarizedLight => "solarized-light",
            Theme::NordLight => "nord",
            Theme::Dark => "night",
            Theme::OneDark => "one-dark",
            Theme::Dracula => "dracula",
            Theme::CatppuccinMocha => "catppuccin-mocha",
            Theme::Nord => "nord-dark",
            Theme::GruvboxDark => "gruvbox-dark",
            Theme::SolarizedDark => "solarized-dark",
        }
    }

    pub const fn editor_theme(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::CatppuccinLatte => "catppuccin_latte",
            Theme::SolarizedLight => "solarized_light",
            Theme::NordLight => "nord_light",
            Theme::Dark => "dark",
            Theme::OneDark => "one_dark",
            Theme::Dracula => "dracula",
            Theme::CatppuccinMocha => "catppuccin_mocha",
            Theme::Nord => "nord",
            Theme::GruvboxDark => "gruvbox_dark",
            Theme::SolarizedDark => "solarized_dark",
        }
    }

    pub fn label(self, locale: Locale) -> String {
        match self {
            Theme::Light => td_display!(locale, theme_light).to_string(),
            Theme::CatppuccinLatte => "Catppuccin Latte".to_owned(),
            Theme::SolarizedLight => "Solarized Light".to_owned(),
            Theme::NordLight => "Nord Light".to_owned(),
            Theme::Dark => td_display!(locale, theme_dark).to_string(),
            Theme::OneDark => "One Dark".to_owned(),
            Theme::Dracula => "Dracula".to_owned(),
            Theme::CatppuccinMocha => "Catppuccin Mocha".to_owned(),
            Theme::Nord => "Nord".to_owned(),
            Theme::GruvboxDark => "Gruvbox Dark".to_owned(),
            Theme::SolarizedDark => "Solarized Dark".to_owned(),
        }
    }
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

const fn default_tab_size() -> u8 {
    4
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub(crate) struct StoredSettings {
    theme: Option<Theme>,
    keyboard_mode: KeyboardMode,
    input_mode: InputMode,
    #[serde(default = "default_tab_size")]
    pub(crate) tab_size: u8,
    persist_storage: bool,
    editor_width_percent: f32,
    #[serde(default = "default_input_height_percent")]
    input_height_percent: f32,
    mem_limit: Option<u32>,
    time_limit: Option<f64>,
    pub(crate) default_language: Option<String>,
}

impl Default for StoredSettings {
    fn default() -> Self {
        Self {
            theme: None,
            keyboard_mode: KeyboardMode::Standard,
            input_mode: InputMode::Batch,
            tab_size: default_tab_size(),
            persist_storage: false,
            editor_width_percent: 65.0,
            input_height_percent: default_input_height_percent(),
            mem_limit: None,
            time_limit: None,
            default_language: None,
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
    pub tab_size: Signal<u8>,
    pub persist_storage: Signal<bool>,
    pub mem_limit: Signal<Option<u32>>,
    pub time_limit: Signal<Option<f64>>,
    pub default_language: Signal<Option<String>>,
}

impl SettingsProvider {
    pub fn install(mut default_settings: StoredSettings, config_default_language: Option<String>) {
        if default_settings.default_language.is_none() {
            default_settings.default_language = config_default_language.clone();
        }
        let (read, write, _) = use_local_storage_with_options::<StoredSettings, JsonSerdeCodec>(
            "wasm_ide_settings",
            UseStorageOptions::default().initial_value(default_settings),
        );
        if let Some(cfg_lang) = &config_default_language
            && read.get_untracked().default_language.is_none()
        {
            write.update(|s| s.default_language = Some(cfg_lang.clone()));
        }
        let default_language = Memo::new(move |_| {
            read.get()
                .default_language
                .or_else(|| config_default_language.clone())
                .filter(|lang| backend::languages().iter().any(|l| &l.name == lang))
        });
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
                let _ = root.set_attribute("data-theme", theme.get().daisy_theme());
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
            tab_size: Memo::new(move |_| read.get().tab_size.clamp(1, 16)).into(),
            persist_storage: Memo::new(move |_| read.get().persist_storage).into(),
            mem_limit: Memo::new(move |_| read.get().mem_limit).into(),
            time_limit: Memo::new(move |_| read.get().time_limit).into(),
            default_language: default_language.into(),
        });
    }

    pub fn set_editor_width(self, val: f32) {
        self.write.update(|v| {
            v.editor_width_percent = val.clamp(MIN_EDITOR_WIDTH, MAX_EDITOR_WIDTH);
        });
    }

    pub fn set_input_height(self, val: f32) {
        self.write.update(|v| {
            v.input_height_percent = val.clamp(MIN_INPUT_HEIGHT, MAX_INPUT_HEIGHT);
        });
    }

    pub fn set_input_mode(self, input_mode: InputMode) {
        self.write.update(|v| v.input_mode = input_mode);
    }

    pub fn set_default_language(self, language: String) {
        self.write.update(|v| v.default_language = Some(language));
    }

    pub fn set_keyboard_mode(self, keyboard_mode: KeyboardMode) {
        self.write.update(|v| v.keyboard_mode = keyboard_mode);
    }

    pub fn set_theme(self, theme: Option<Theme>) {
        self.write.update(|v| v.theme = theme);
    }
}

pub fn use_settings() -> SettingsProvider {
    expect_context()
}

pub const MIN_EDITOR_WIDTH: f32 = 30.0;
pub const MAX_EDITOR_WIDTH: f32 = 80.0;
pub const MIN_INPUT_HEIGHT: f32 = 20.0;
pub const MAX_INPUT_HEIGHT: f32 = 80.0;

#[component]
pub fn Settings() -> impl IntoView {
    let i18n = use_i18n();
    let open = RwSignal::new(false);

    let settings = use_settings();
    let keyboard_mode = settings.keyboard_mode;
    let set_kb_mode = move |kb_mode| settings.set_keyboard_mode(kb_mode);

    let locale_value = (
        Signal::derive(move || i18n.get_locale()),
        SignalSetter::map(move |new_locale: Locale| {
            i18n.set_locale(new_locale);
        }),
    );

    let content = NodeRef::new();
    let theme_dropdown_open = RwSignal::new(false);

    let _ = on_click_outside(content, move |_| {
        theme_dropdown_open.set(false);
        open.set(false);
    });
    let _ = use_event_listener(use_document(), keydown, move |evt| {
        if evt.key_code() == 27 {
            if theme_dropdown_open.get_untracked() {
                theme_dropdown_open.set(false);
            } else {
                open.set(false);
            }
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

                        <DefaultLanguageControl />

                        <div class="grid grid-cols-1 sm:grid-cols-3 items-center gap-2">
                            <label class="text-sm font-medium">{t!(i18n, keyboard_mode)}</label>
                            <div class="sm:col-span-2">
                                <EnumSelect value=(keyboard_mode, SignalSetter::map(set_kb_mode)) />
                            </div>
                        </div>

                        <TabSizeControl />

                        <ThemeControl dropdown_open=theme_dropdown_open />
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
fn DefaultLanguageControl() -> impl IntoView {
    let i18n = use_i18n();
    let settings = use_settings();

    view! {
        <div class="grid grid-cols-1 sm:grid-cols-3 items-center gap-2">
            <label class="text-sm font-medium">{t!(i18n, default_programming_language)}</label>
            <div class="sm:col-span-2">
                <select
                    class="select select-bordered select-sm w-auto min-w-40"
                    prop:value=move || settings.default_language.get().unwrap_or_default()
                    on:change:target=move |ev| {
                        let val = ev.target().value();
                        if !val.is_empty() {
                            settings.set_default_language(val);
                        }
                    }
                >
                    <For each=move || backend::languages() key=|l| l.name.clone() let:lang>
                        <option selected=move || {
                            settings.default_language.get().as_deref() == Some(&lang.name)
                        }>{lang.name.clone()}</option>
                    </For>
                </select>
            </div>
        </div>
    }
}

#[component]
fn TabSizeControl() -> impl IntoView {
    let i18n = use_i18n();
    let settings = use_settings();

    let set_tab_size = move |size: u8| {
        settings.write.update(|v| v.tab_size = size);
    };

    view! {
        <div class="grid grid-cols-1 sm:grid-cols-3 items-center gap-2">
            <label class="text-sm font-medium">{t!(i18n, tab_size)}</label>
            <div class="sm:col-span-2">
                <div class="join">
                    {[2u8, 4, 8]
                        .into_iter()
                        .map(|size| {
                            view! {
                                <button
                                    type="button"
                                    class=move || {
                                        if settings.tab_size.get() == size {
                                            "btn btn-sm join-item btn-primary font-mono"
                                        } else {
                                            "btn btn-sm join-item btn-outline border-base-300 font-mono"
                                        }
                                    }
                                    on:click=move |_| set_tab_size(size)
                                >
                                    {t!(i18n, tab_spaces, count = size)}
                                </button>
                            }
                        })
                        .collect_view()}
                </div>
            </div>
        </div>
    }
}

#[component]
fn ThemeSwatch(#[prop(into)] daisy_theme: Signal<&'static str>) -> impl IntoView {
    view! {
        <span
            attr:data-theme=move || daisy_theme.get()
            class="inline-flex items-center gap-0.5 rounded-md border border-base-300 bg-base-100 px-1.5 py-1 shrink-0"
        >
            <span class="w-2 h-2 rounded-full bg-primary" />
            <span class="w-2 h-2 rounded-full bg-secondary" />
            <span class="w-2 h-2 rounded-full bg-accent" />
        </span>
    }
}

#[component]
fn ThemeControl(dropdown_open: RwSignal<bool>) -> impl IntoView {
    let settings = use_settings();
    let read = settings.read;
    let resolved_theme = settings.theme;
    let i18n = use_i18n();
    let selected = Signal::derive(move || read.get().theme);
    let set_theme = move |theme| settings.set_theme(theme);

    let preferred_dark = leptos_use::use_preferred_dark();
    let system_icon = Signal::derive(move || {
        if preferred_dark.get() {
            icondata::BiMoonSolid
        } else {
            icondata::BiSunSolid
        }
    });

    let dropdown_ref = NodeRef::new();
    let _ = on_click_outside(dropdown_ref, move |_| dropdown_open.set(false));

    let active_daisy = Signal::derive(move || resolved_theme.get().daisy_theme());
    let active_label = Signal::derive(move || {
        let locale = i18n.get_locale();
        match selected.get() {
            None => format!(
                "{} ({})",
                td_string!(locale, theme_system),
                resolved_theme.get().label(locale)
            ),
            Some(preset) => preset.label(locale),
        }
    });

    let render_preset = move |preset: Theme| {
        let is_selected = Signal::derive(move || selected.get() == Some(preset));
        let label = Signal::derive(move || preset.label(i18n.get_locale()));
        view! {
            <button
                type="button"
                class=move || {
                    if is_selected.get() {
                        "flex items-center justify-between gap-2 px-2.5 py-1.5 rounded-lg text-xs font-medium bg-primary/15 text-base-content ring-1 ring-primary/50 cursor-pointer text-left transition-colors"
                    } else {
                        "flex items-center justify-between gap-2 px-2.5 py-1.5 rounded-lg text-xs text-base-content/85 hover:bg-base-200 cursor-pointer text-left transition-colors"
                    }
                }
                on:click=move |_| set_theme(Some(preset))
            >
                <span class="flex items-center gap-2 min-w-0">
                    <ThemeSwatch daisy_theme=Signal::derive(move || preset.daisy_theme()) />
                    <span class="truncate">{label}</span>
                </span>
                <Show when=move || is_selected.get()>
                    <Icon attr:class="w-3.5 h-3.5 text-primary shrink-0" icon=icondata::BiCheckRegular />
                </Show>
            </button>
        }
    };

    view! {
        <div class="grid grid-cols-1 sm:grid-cols-3 items-center gap-2">
            <label class="text-sm font-medium">{t!(i18n, theme)}</label>
            <div class="sm:col-span-2 relative" node_ref=dropdown_ref>
                <button
                    type="button"
                    class="btn btn-sm btn-outline border-base-300 w-full justify-between font-normal px-3"
                    on:click=move |_| dropdown_open.update(|v| *v = !*v)
                >
                    <span class="flex items-center gap-2 min-w-0">
                        <ThemeSwatch daisy_theme=active_daisy />
                        <Show when=move || selected.get().is_none()>
                            <Icon attr:class="w-3.5 h-3.5 opacity-75 shrink-0" icon=system_icon />
                        </Show>
                        <span class="truncate">{active_label}</span>
                    </span>
                    <Icon attr:class="w-4 h-4 opacity-60 shrink-0" icon=icondata::BiChevronDownRegular />
                </button>
                <Show when=move || dropdown_open.get()>
                    <div class="absolute bottom-full left-0 right-0 mb-1.5 z-50 rounded-xl border border-base-300 bg-base-100 shadow-xl p-2 max-h-60 overflow-y-auto flex flex-col gap-1.5">
                        <button
                            type="button"
                            class=move || {
                                if selected.get().is_none() {
                                    "flex items-center justify-between gap-2 px-2.5 py-1.5 rounded-lg text-xs font-medium bg-primary/15 text-base-content ring-1 ring-primary/50 cursor-pointer transition-colors"
                                } else {
                                    "flex items-center justify-between gap-2 px-2.5 py-1.5 rounded-lg text-xs text-base-content/85 hover:bg-base-200 cursor-pointer transition-colors"
                                }
                            }
                            on:click=move |_| set_theme(None)
                        >
                            <span class="flex items-center gap-2">
                                <Icon attr:class="w-4 h-4 shrink-0" icon=system_icon />
                                <span>{t!(i18n, theme_system)}</span>
                            </span>
                            <Show when=move || selected.get().is_none()>
                                <Icon attr:class="w-3.5 h-3.5 text-primary shrink-0" icon=icondata::BiCheckRegular />
                            </Show>
                        </button>

                        <div class="px-2 pt-1 text-[10px] font-semibold uppercase tracking-wider text-base-content/50 flex items-center gap-1.5">
                            <Icon attr:class="w-3 h-3" icon=icondata::BiSunSolid />
                            {t!(i18n, theme_light)}
                        </div>
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-1">
                            {Theme::LIGHT_PRESETS.iter().copied().map(render_preset).collect_view()}
                        </div>

                        <div class="px-2 pt-1 text-[10px] font-semibold uppercase tracking-wider text-base-content/50 flex items-center gap-1.5">
                            <Icon attr:class="w-3 h-3" icon=icondata::BiMoonSolid />
                            {t!(i18n, theme_dark)}
                        </div>
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-1">
                            {Theme::DARK_PRESETS.iter().copied().map(render_preset).collect_view()}
                        </div>
                    </div>
                </Show>
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
