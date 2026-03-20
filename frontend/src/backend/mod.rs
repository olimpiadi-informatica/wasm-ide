use std::sync::Arc;

use common::{Language, WorkerRequest, WorkerResponse};

mod combined;
mod remote;
mod worker;

pub use combined::*;
pub use remote::*;
pub use worker::*;

pub type Callback = Arc<dyn Fn(WorkerResponse) + Send + Sync>;

pub trait Backend {
    fn languages(&self) -> &[Language];
    fn set_callback(&self, callback: Callback);
    fn send_message(self: Arc<Self>, msg: WorkerRequest);
    fn has_dynamic_io(&self, lang: &str) -> bool;
}

pub type DynBackend = Arc<dyn Backend + Send + Sync>;
