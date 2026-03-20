use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use common::{Language, WorkerExecRequest, WorkerLSRequest, WorkerRequest};

use crate::backend::{Backend, Callback, DynBackend};

pub struct CombinedBackend {
    languages: Vec<Language>,
    a: DynBackend,
    b: DynBackend,
    active: AtomicBool,
}

impl CombinedBackend {
    pub fn new(a: DynBackend, b: DynBackend) -> Arc<Self> {
        let languages = a.languages().iter().chain(b.languages()).cloned().collect();
        let active = AtomicBool::new(false);
        Arc::new(Self {
            languages,
            a,
            b,
            active,
        })
    }

    fn which(&self, lang: &str) -> Option<bool> {
        if self.a.languages().iter().any(|l| l.name == lang) {
            Some(false)
        } else if self.b.languages().iter().any(|l| l.name == lang) {
            Some(true)
        } else {
            None
        }
    }
}

impl Backend for CombinedBackend {
    fn languages(&self) -> &[Language] {
        &self.languages
    }

    fn set_callback(&self, callback: Callback) {
        self.a.set_callback(callback.clone());
        self.b.set_callback(callback);
    }

    fn send_message(self: Arc<Self>, msg: WorkerRequest) {
        let lang = match &msg {
            WorkerRequest::Execution(WorkerExecRequest::CompileAndRun { language, .. }) => {
                Some(language)
            }
            WorkerRequest::LS(WorkerLSRequest::Start(language)) => Some(language),
            _ => None,
        };
        if let Some(lang) = lang {
            self.active.store(
                self.which(lang).expect("unknown language"),
                Ordering::Relaxed,
            );
        }
        let active = self.active.load(Ordering::Relaxed);
        match active {
            false => self.a.clone().send_message(msg),
            true => self.b.clone().send_message(msg),
        }
    }

    fn has_dynamic_io(&self, lang: &str) -> bool {
        let which = self.which(lang).expect("unknown language");
        match which {
            false => self.a.has_dynamic_io(lang),
            true => self.b.has_dynamic_io(lang),
        }
    }
}
