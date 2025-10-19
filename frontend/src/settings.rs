use leptos::{prelude::*, reactive::wrappers::write::SignalSetter};
use thaw::{
    Button, ButtonAppearance, ButtonShape, ButtonSize, Dialog, DialogBody, DialogContent,
    DialogSurface, DialogTitle,
};

use crate::{enum_select::EnumSelect, i18n::*, theme::ThemeSelector, KeyboardMode};

#[component]
pub fn Settings(kb_mode: RwSignal<KeyboardMode>) -> impl IntoView {
    let open = RwSignal::new(false);

    let theme_selector = ThemeSelector();

    view! {
        <Button
            appearance=ButtonAppearance::Subtle
            shape=ButtonShape::Circular
            icon=icondata::LuSettings
            size=ButtonSize::Large
            on_click=move |_| open.set(true)
        />
        <Dialog open>
            <DialogSurface attr:style="width: fit-content;">
                <DialogBody>
                    <DialogTitle>"Settings"</DialogTitle>
                    <DialogContent>
                        <div style="display: grid; grid-template-columns: repeat(2, 1fr); gap: 8px 12px; align-items: center;">
                            <span>"Theme"</span>
                            {theme_selector}

                            <span>"Language"</span>
                            <LocaleSelector />

                            <span>"Keyboard mode"</span>
                            <KbModeSelector kb_mode />
                        </div>
                    </DialogContent>
                </DialogBody>
            </DialogSurface>
        </Dialog>
    }
}

#[component]
fn LocaleSelector() -> impl IntoView {
    fn locale_name(locale: Locale) -> &'static str {
        match locale {
            Locale::en => "English",
            Locale::it => "Italiano",
            Locale::es => "Español",
            Locale::ca => "Català",
        }
    }

    let i18n = use_i18n();

    let mut options = Locale::get_all()
        .iter()
        .map(|&x| (x, Signal::stored(locale_name(x).to_string())))
        .collect::<Vec<_>>();
    options.sort_by_key(|&(loc, _)| locale_name(loc));

    view! {
        <EnumSelect
            class="locale-selector"
            value=(
                Signal::derive(move || i18n.get_locale()),
                SignalSetter::map(move |new_locale: Locale| {
                    i18n.set_locale(new_locale);
                }),
            )
            options
        />
    }
}

#[component]
fn KbModeSelector(kb_mode: RwSignal<KeyboardMode>) -> impl IntoView {
    fn kb_mode_string(locale: Locale, kb_mode: KeyboardMode) -> String {
        match kb_mode {
            KeyboardMode::Vim => td_display!(locale, vim_mode),
            KeyboardMode::Emacs => td_display!(locale, emacs_mode),
            KeyboardMode::Standard => td_display!(locale, standard_mode),
        }
        .to_string()
    }

    let i18n = use_i18n();

    let options = [
        KeyboardMode::Standard,
        KeyboardMode::Vim,
        KeyboardMode::Emacs,
    ]
    .into_iter()
    .map(|mode| {
        (
            mode,
            Signal::derive(move || kb_mode_string(i18n.get_locale(), mode)),
        )
    })
    .collect::<Vec<_>>();

    view! { <EnumSelect class="kb-selector" value=(kb_mode.into(), kb_mode.into()) options /> }
}
