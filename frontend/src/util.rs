use js_sys::Uint8Array;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Blob, HtmlAnchorElement, Url};

use crate::{backend, settings::InputMode};

pub fn download(name: &str, data: &[u8]) {
    let array8 = Uint8Array::from(data);
    let array = js_sys::Array::of1(&array8);
    let blob = Blob::new_with_u8_array_sequence(&array).unwrap();
    let url = Url::create_object_url_with_blob(&blob).unwrap();
    let a = document()
        .create_element("a")
        .unwrap()
        .dyn_into::<HtmlAnchorElement>()
        .unwrap();
    a.set_download(name);
    a.set_href(&url);
    a.click();
}

#[component]
pub fn Icon(#[prop(into)] icon: Signal<icondata::Icon>) -> impl IntoView {
    view! {
        <svg
            inner_html=move || icon.get().data
            viewBox=move || icon.get().view_box
            stroke-linecap=move || icon.get().stroke_linecap
            stroke-linejoin=move || icon.get().stroke_linejoin
            stroke-width=move || icon.get().stroke_width
            stroke=move || icon.get().stroke
            width="1em"
            height="1em"
            x=move || icon.get().x
            y=move || icon.get().y
            fill=move || icon.get().fill.unwrap_or("currentColor")
        />
    }
}

pub fn get_input_mode(input_mode: InputMode, lang: &str) -> InputMode {
    match backend::for_lang(lang).has_dynamic_io() {
        true => input_mode,
        false => InputMode::Batch,
    }
}
