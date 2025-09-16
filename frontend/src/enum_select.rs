use leptos::prelude::*;
use thaw::Select;

pub fn enum_select<T>(
    class: &str,
    init: T,
    options: Vec<(T, Signal<String>)>,
) -> (Signal<T>, impl IntoView)
where
    T: Sized + Clone + PartialEq + Eq + Send + Sync + 'static,
{
    let (values, names): (Vec<_>, Vec<_>) = options.into_iter().unzip();

    let init = values
        .iter()
        .position(|v| v == &init)
        .expect("Initial value must be one of the enum variants");

    let value_str = RwSignal::new(init.to_string());
    let value = Signal::derive(move || {
        value_str.with(|s| {
            let idx = s
                .parse::<usize>()
                .expect("Value string should be a valid index");
            values[idx].clone()
        })
    });

    let view = view! {
        <Select value=value_str class>
            {names.into_iter().enumerate().map(|(i, n)| {
                let id = i.to_string();
                view! { <option value=id.clone() selected=move || value_str.get() == id>{n}</option> }
            }).collect::<Vec<_>>()}
        </Select>
    };

    (value, view)
}
