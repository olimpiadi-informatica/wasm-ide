use common::Language;
use leptos::prelude::*;
use leptos::reactive::wrappers::write::SignalSetter;
use strum::VariantArray;

use crate::i18n::{use_i18n, Locale};

pub trait DisplayLocalized {
    fn to_localized_string(&self, locale: Locale) -> String;
}

impl VariantArray for Locale {
    const VARIANTS: &'static [Self] =
        &[Locale::en, Locale::it, Locale::es, Locale::ca, Locale::vec];
}

impl DisplayLocalized for Locale {
    fn to_localized_string(&self, _locale: Locale) -> String {
        match self {
            Locale::en => "English",
            Locale::it => "Italiano",
            Locale::es => "Español",
            Locale::ca => "Català",
            Locale::vec => "Vèneto",
        }
        .to_owned()
    }
}

impl DisplayLocalized for Language {
    fn to_localized_string(&self, _locale: Locale) -> String {
        self.to_string()
    }
}

#[component]
pub fn EnumSelect<T>(
    #[prop(into, name = "value")] (getter, setter): (Signal<T>, SignalSetter<T>),
) -> impl IntoView
where
    T: DisplayLocalized + VariantArray + Send + Sync + Clone + PartialEq + 'static,
{
    let i18n = use_i18n();

    let getter_str = Signal::derive({
        move || {
            let val = getter.get();
            T::VARIANTS
                .iter()
                .cloned()
                .enumerate()
                .find(|(_, t)| *t == val)
                .unwrap()
                .0
                .to_string()
        }
    });

    let setter_str = SignalSetter::<String>::map(move |s: String| {
        let idx = s
            .parse::<usize>()
            .expect("Value string should be a valid index");
        if getter.get_untracked() != T::VARIANTS[idx] {
            setter.set(T::VARIANTS[idx].clone());
        }
    });

    view! {
        <div class:select>
            <select
                on:change:target=move |ev| {
                    setter_str.set(ev.target().value());
                }
                prop:value=move || getter_str.get()
            >
                <For each=move || 0..T::VARIANTS.len() key=|i| *i let:i>
                    <option value=i.to_string() selected=move || getter.get() == T::VARIANTS[i]>
                        {move || T::VARIANTS[i].to_localized_string(i18n.get_locale())}
                    </option>
                </For>
            </select>
        </div>
    }
}
