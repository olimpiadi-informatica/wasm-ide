use std::{cell::RefCell, rc::Rc};

use anyhow::{anyhow, Result};
use futures::{
    channel::oneshot::{channel, Receiver, Sender},
    lock::Mutex,
};
use js_sys::{
    Object, SharedArrayBuffer,
    WebAssembly::{Memory, Module},
};
use wasm_bindgen::{closure::Closure, JsCast};
use web_sys::{Worker, WorkerOptions, WorkerType};
use web_time::Instant;

use super::{syscall, Fs, Inode, Pipe};

type WriteFn = Rc<dyn Fn(&[u8]) -> usize>;

pub enum FdEntry {
    WriteFn(WriteFn),
    Data {
        data: Vec<u8>,
        offset: usize,
    },
    Dir(Inode),
    /// inode, offset, append
    File(Inode, usize, bool),
    Pipe(Rc<Pipe>),
}

pub struct Process {
    pub module: Module,
    pub memory: Memory,
    pub start_instant: Instant,
    pub args: Vec<Vec<u8>>,
    pub env: Vec<Vec<u8>>,
    pub termiation_send: Mutex<Sender<()>>,
    pub inner: RefCell<ProcessInner>,
}

impl Drop for Process {
    fn drop(&mut self) {
        self.kill(StatusCode::Signaled);
    }
}

pub struct ProcessInner {
    pub fds: Vec<Option<FdEntry>>,
    pub status_code: StatusCode,
    pub threads: Vec<(Worker, SharedArrayBuffer)>,
    pub termination_recv: Receiver<()>,
    pub fs: Fs,
}

impl ProcessInner {
    pub fn add_fd(&mut self, entry: FdEntry) -> u32 {
        for fd in 0..self.fds.len() {
            if self.fds[fd].is_none() {
                self.fds[fd] = Some(entry);
                return fd as u32;
            }
        }
        self.fds.push(Some(entry));
        self.fds.len() as u32 - 1
    }
}

impl Process {
    pub fn kill(&self, status_code: StatusCode) {
        let mut inner = self.inner.borrow_mut();
        inner.status_code = status_code;
        for (worker, _) in inner.threads.drain(..) {
            worker.terminate();
        }
        inner.termination_recv.close();
    }

    pub async fn wait(&self) -> StatusCode {
        let mut l = self.termiation_send.lock().await;
        l.cancellation().await;
        self.inner.borrow().status_code.clone()
    }

    pub fn spawn_thread(self: &Rc<Self>, arg: Option<i32>) -> u32 {
        let tid = self.inner.borrow().threads.len() as u32 + 1;

        let channel = SharedArrayBuffer::new(4);

        let path = wasm_bindgen::link_to!(module = "/src/os/start_proc.js");
        let options = WorkerOptions::default();
        options.set_type(WorkerType::Module);
        let worker = Worker::new_with_options(&path, &options).expect("couldn't start thread");

        let msg = Object::new();
        js_sys::Reflect::set(&msg, &"module".into(), &self.module).expect("could not set module");
        js_sys::Reflect::set(&msg, &"memory".into(), &self.memory).expect("could not set memory");
        js_sys::Reflect::set(&msg, &"channel".into(), &channel).expect("could not set channel");
        if let Some(arg) = arg {
            js_sys::Reflect::set(&msg, &"tid".into(), &tid.into()).expect("could not set argument");
            js_sys::Reflect::set(&msg, &"arg".into(), &arg.into()).expect("could not set argument");
        }
        worker
            .post_message(&msg)
            .expect("failed sending init message to worker");

        let proc = Rc::downgrade(self);
        worker.set_onmessage(Some(
            Closure::<dyn Fn(_)>::new(move |msg| {
                if let Some(proc) = proc.upgrade() {
                    syscall::handle_message(proc.clone(), tid, msg);
                }
            })
            .into_js_value()
            .unchecked_ref(),
        ));

        self.inner.borrow_mut().threads.push((worker, channel));

        tid
    }
}

#[derive(Clone)]
pub struct ProcessHandle {
    pub proc: Rc<Process>,
}

#[derive(Default)]
pub struct Builder {
    fs: Option<Fs>,
    stdin: Option<FdEntry>,
    stdout: Option<FdEntry>,
    stderr: Option<FdEntry>,
    env: Vec<Vec<u8>>,
}

impl Builder {
    pub fn fs(mut self, fs: Fs) -> Self {
        self.fs = Some(fs);
        self
    }

    pub fn stdin(mut self, stdin: FdEntry) -> Self {
        self.stdin = Some(stdin);
        self
    }

    pub fn stdout(mut self, stdout: FdEntry) -> Self {
        self.stdout = Some(stdout);
        self
    }

    pub fn stderr(mut self, stderr: FdEntry) -> Self {
        self.stderr = Some(stderr);
        self
    }

    pub fn env(mut self, mut env: Vec<u8>) -> Self {
        env.push(0);
        self.env.push(env);
        self
    }

    pub fn spawn_with_module(self, module: Module, args: Vec<Vec<u8>>) -> ProcessHandle {
        let imports_memory = Module::imports(&module).iter().any(|import| {
            let kind =
                js_sys::Reflect::get(&import, &"kind".into()).expect("could not get import kind");
            let module = js_sys::Reflect::get(&import, &"module".into())
                .expect("could not get import module");
            let name =
                js_sys::Reflect::get(&import, &"name".into()).expect("could not get import name");
            kind.as_string() == Some("memory".to_string())
                && module.as_string() == Some("env".to_string())
                && name.as_string() == Some("memory".to_string())
        });
        assert!(imports_memory);

        // TODO: get the opts from the module
        let mem_opts = Object::new();
        js_sys::Reflect::set(&mem_opts, &"initial".into(), &640.into())
            .expect("could not set initial memory size");
        js_sys::Reflect::set(&mem_opts, &"maximum".into(), &65536.into())
            .expect("could not set maximum memory size");
        js_sys::Reflect::set(&mem_opts, &"shared".into(), &true.into())
            .expect("could not set shared memory option");
        let memory = Memory::new(&mem_opts).expect("could not create memory");

        let start_instant = Instant::now();

        let args = args
            .into_iter()
            .map(|mut arg| {
                arg.push(0);
                arg
            })
            .collect::<Vec<_>>();

        let fs = self.fs.unwrap_or_default();

        let fds = vec![
            self.stdin,
            self.stdout,
            self.stderr,
            Some(FdEntry::Dir(fs.root())),
        ];

        let (termination_send, termination_recv) = channel();

        let inner = ProcessInner {
            fds,
            status_code: StatusCode::Signaled,
            threads: vec![],
            termination_recv,
            fs,
        };

        let proc = Rc::new(Process {
            module,
            memory,
            start_instant,
            args,
            env: self.env,
            termiation_send: Mutex::new(termination_send),
            inner: RefCell::new(inner),
        });

        proc.spawn_thread(None);

        ProcessHandle { proc }
    }

    pub fn spawn_with_code(self, code: &[u8], args: Vec<Vec<u8>>) -> ProcessHandle {
        let uint8array = js_sys::Uint8Array::new_with_length(code.len() as u32);
        uint8array.copy_from(code);
        let module = Module::new(&uint8array).expect("could not create module from wasm bytes");
        self.spawn_with_module(module, args)
    }

    pub fn spawn_with_path(self, path: &[u8], args: Vec<Vec<u8>>) -> ProcessHandle {
        let code = self.fs.as_ref().unwrap().get_file_with_path(path).unwrap();
        self.spawn_with_code(&code, args)
    }
}

impl ProcessHandle {
    pub fn builder() -> Builder {
        Builder::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum StatusCode {
    Exited(u32),
    Signaled,
    RuntimeError(String),
}

impl StatusCode {
    pub fn check_success(&self) -> Result<()> {
        match self {
            StatusCode::Exited(0) => Ok(()),
            StatusCode::Exited(code) => Err(anyhow!("Process exited with non-zero code: {}", code)),
            StatusCode::Signaled => Err(anyhow!("Process was killed by a signal")),
            StatusCode::RuntimeError(msg) => {
                Err(anyhow!("Process encountered a runtime error: {}", msg))
            }
        }
    }
}
