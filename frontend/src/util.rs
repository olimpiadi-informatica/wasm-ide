use js_sys::Uint8Array;
use leptos::prelude::document;
use wasm_bindgen::JsCast;
use web_sys::{Blob, HtmlAnchorElement, Url};

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
