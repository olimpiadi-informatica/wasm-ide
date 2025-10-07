use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use thaw::{Button, ButtonAppearance, Icon, Text, Theme};

use crate::{load, save};

#[component]
pub fn ThemeSelector() -> impl IntoView {
    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    enum ThemePlus {
        System,
        Light,
        Dark,
    }

    let preferred_dark = leptos_use::use_preferred_dark();
    let theme_plus = RwSignal::new(load("theme").unwrap_or(ThemePlus::System));
    let theme = Theme::use_rw_theme();

    Effect::new(move |_| {
        let new_theme = match theme_plus.get() {
            ThemePlus::System => match preferred_dark.get() {
                true => Theme::dark(),
                false => Theme::light(),
            },
            ThemePlus::Light => Theme::light(),
            ThemePlus::Dark => Theme::dark(),
        };
        if new_theme.name != theme.get_untracked().name {
            theme.set(new_theme);
        }
    });

    let theme_name_and_icon = Memo::new(move |_| match theme_plus.get() {
        ThemePlus::System => match preferred_dark.get() {
            true => ("System", icondata::BiMoonSolid),
            false => ("System", icondata::BiSunSolid),
        },
        ThemePlus::Light => ("Light", icondata::BiSunSolid),
        ThemePlus::Dark => ("Dark", icondata::BiMoonSolid),
    });
    let change_theme = move |_| {
        let new_theme = match theme_plus.get_untracked() {
            ThemePlus::System => ThemePlus::Light,
            ThemePlus::Light => ThemePlus::Dark,
            ThemePlus::Dark => ThemePlus::System,
        };
        save("theme", &new_theme);
        theme_plus.set(new_theme);
    };

    view! {
        <Button appearance=ButtonAppearance::Subtle on_click=change_theme>
            {move || {
                let (name, icon) = theme_name_and_icon.get();
                view! {
                    <Icon icon style="padding: 0 5px 0 0;" width="1.5em" height="1.5em"/>
                    <Text>{name}</Text>
                }
            }}
        </Button>
    }
}
