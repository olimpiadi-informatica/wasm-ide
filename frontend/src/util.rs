use std::borrow::Cow;

use js_sys::Uint8Array;
use leptos::prelude::*;
use serde::{de::DeserializeOwned, Serialize};
use wasm_bindgen::JsCast;
use web_sys::{Blob, HtmlAnchorElement, Url};

use crate::{editor::EditorText, LargeFileSet};

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

pub trait Stringifiable: Sized {
    fn stringify(&self) -> Cow<'_, str>;
    fn from_string(data: String) -> Option<Self>;
}

impl Stringifiable for EditorText {
    fn stringify(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.text())
    }
    fn from_string(data: String) -> Option<EditorText> {
        Some(EditorText::from_text(data))
    }
}

impl<T: Serialize + DeserializeOwned> Stringifiable for T {
    fn stringify(&self) -> Cow<'_, str> {
        Cow::Owned(serde_json::to_string(self).expect("serialization error"))
    }
    fn from_string(data: String) -> Option<Self> {
        serde_json::from_str(&data).ok()
    }
}

pub fn save<T: Stringifiable>(key: &str, value: &T) {
    let s = value.stringify();
    let large_files = expect_context::<RwSignal<LargeFileSet>>();
    if s.len() >= 3_000_000 {
        large_files.update(|x| {
            x.0.insert(key.to_owned());
        });
        return;
    }
    large_files.update(|x| {
        x.0.remove(key);
    });
    window()
        .local_storage()
        .expect("no local storage")
        .unwrap()
        .set(key, &s)
        .expect("could not save data");
}

pub fn load<T: Stringifiable>(key: &str) -> Option<T> {
    window()
        .local_storage()
        .expect("no local storage")
        .unwrap()
        .get(key)
        .expect("error fetching from local storage")
        .and_then(|x| T::from_string(x))
}
