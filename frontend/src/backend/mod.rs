use std::{
    collections::HashSet,
    sync::{Arc, Mutex, MutexGuard},
};

use common::{Language, WorkerRequest, WorkerResponse};

mod js;
mod remote;
mod worker;

pub use js::*;
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

fn backends() -> MutexGuard<'static, Vec<DynBackend>> {
    BACKENDS
        .try_lock()
        .expect("Failed to acquire backends lock")
}

pub fn register_backend(backend: DynBackend) {
    backends().push(backend);
}

pub fn for_lang(lang: &str) -> DynBackend {
    backends()
        .iter()
        .find(|b| b.languages().iter().any(|l| l.name == lang))
        .cloned()
        .expect("No backend found for language")
}

pub fn set_callback(callback: Callback) {
    for backend in backends().iter() {
        backend.set_callback(callback.clone());
    }
}

pub fn languages() -> Vec<Language> {
    let mut seen = HashSet::new();
    backends()
        .iter()
        .flat_map(|b| b.languages())
        .filter(|l| seen.insert(l.name.clone()))
        .cloned()
        .collect()
}
