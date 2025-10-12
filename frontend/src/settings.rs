use leptos::prelude::*;
use thaw::{
    Button, ButtonAppearance, ButtonShape, ButtonSize, Dialog, DialogBody, DialogContent,
    DialogSurface, DialogTitle,
};

use crate::{enum_select::enum_select, i18n::*, locale_name, theme::ThemeSelector};

#[component]
pub fn Settings(kb_mode_select: impl IntoView + 'static) -> impl IntoView {
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
                            <span>"Theme: "</span>
                            {theme_selector}

                            <span>"Language: "</span>
                            <LocaleSelector />

                            <span>"Keyboard mode: "</span>
                            {kb_mode_select}
                        </div>
                    </DialogContent>
                </DialogBody>
            </DialogSurface>
        </Dialog>
    }
}

#[component]
fn LocaleSelector() -> impl IntoView {
    let i18n = use_i18n();

    let init = i18n.get_locale_untracked();

    let (locale, view) = enum_select(
        "locale-selector",
        init,
        Locale::get_all()
            .iter()
            .map(|&x| (x, Signal::stored(locale_name(x).to_string())))
            .collect::<Vec<_>>(),
    );

    Effect::new(move |_| {
        let loc = locale.get();
        i18n.set_locale(loc);
    });

    view
}
