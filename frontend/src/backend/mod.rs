use std::sync::{Arc, Mutex};

use common::{Language, WorkerRequest, WorkerResponse};

mod remote;
mod worker;

pub use remote::*;
pub use worker::*;

pub type Callback = Arc<dyn Fn(WorkerResponse) + Send + Sync>;

pub trait Backend {
    fn languages(&self) -> &[Language];
    fn set_callback(&self, callback: Callback);
    fn send_message(self: Arc<Self>, msg: WorkerRequest);
    fn has_dynamic_io(&self) -> bool;
}

pub type DynBackend = Arc<dyn Backend + Send + Sync>;

static BACKENDS: Mutex<Vec<DynBackend>> = Mutex::new(Vec::new());

pub fn register_backend(backend: DynBackend) {
    BACKENDS.lock().unwrap().push(backend);
}

pub fn for_lang(lang: &str) -> DynBackend {
    BACKENDS
        .lock()
        .unwrap()
        .iter()
        .find(|b| b.languages().iter().any(|l| l.name == lang))
        .cloned()
        .expect("No backend found for language")
}

pub fn set_callback(callback: Callback) {
    for backend in BACKENDS.lock().unwrap().iter() {
        backend.set_callback(callback.clone());
    }
}

pub fn languages() -> Vec<Language> {
    BACKENDS
        .lock()
        .unwrap()
        .iter()
        .flat_map(|b| b.languages().to_vec())
        .collect()
}
