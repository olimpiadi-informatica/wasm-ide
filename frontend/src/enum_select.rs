use std::hash::Hash;

use leptos::{prelude::*, reactive::wrappers::write::SignalSetter};
use thaw::Select;

#[component]
pub fn EnumSelect<T>(
    #[prop(optional, into)] class: Option<String>,
    #[prop(into, name = "value")] (getter, setter): (Signal<T>, SignalSetter<T>),
    options: Vec<(T, Signal<String>)>,
) -> impl IntoView
where
    T: Clone + Eq + Hash + Send + Sync + 'static,
{
    let values = options.iter().map(|(v, _)| v.clone()).collect::<Vec<_>>();

    let getter_str = Signal::derive({
        let values = values.clone();
        move || {
            let v = getter.get();
            values
                .iter()
                .position(|opt| opt == &v)
                .expect("Value must be one of the enum variants")
                .to_string()
        }
    });

    let setter_str = SignalSetter::<String>::map(move |s: String| {
        let idx = s
            .parse::<usize>()
            .expect("Value string should be a valid index");
        if getter.get_untracked() != values[idx] {
            setter.set(values[idx].clone());
        }
    });

    view! {
        <Select value=(getter_str, setter_str) class>
            <For
                each=move || options.clone().into_iter().enumerate()
                key=|(i, _)| i.clone()
                let:((i, (v, n)))
            >
                <option value=i.to_string() selected=move || getter.get() == v>
                    {n}
                </option>
            </For>
        </Select>
    }
}
