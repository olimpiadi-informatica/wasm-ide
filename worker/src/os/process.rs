use std::{
    cell::{RefCell, RefMut},
    rc::Rc,
};

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

#[derive(Clone)]
pub enum FdEntry {
    WriteFn(WriteFn),
    Data { data: Vec<u8>, offset: usize },
    Dir(Inode),
    File(Inode, usize),
    Pipe(Rc<Pipe>),
}

pub struct Process {
    pub module: Module,
    pub memory: Memory,
    pub start_instant: Instant,
    pub args: Vec<Vec<u8>>,
    pub env: Vec<Vec<u8>>,
    pub fs: Fs,
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

    pub fn get_fd_mut(&self, fd: u32) -> Option<RefMut<'_, FdEntry>> {
        RefMut::filter_map(self.inner.borrow_mut(), |x| {
            x.fds.get_mut(fd as usize).and_then(|x| x.as_mut())
        })
        .ok()
    }

    pub fn add_fd(&self, entry: FdEntry) -> u32 {
        let mut inner = self.inner.borrow_mut();
        for fd in 0..inner.fds.len() {
            if inner.fds[fd].is_none() {
                inner.fds[fd] = Some(entry);
                return fd as u32;
            }
        }
        inner.fds.push(Some(entry));
        inner.fds.len() as u32 - 1
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
        };

        let proc = Rc::new(Process {
            module,
            memory,
            start_instant,
            args,
            env: self.env,
            fs,
            termiation_send: Mutex::new(termination_send),
            inner: RefCell::new(inner),
        });

        proc.spawn_thread(None);

        ProcessHandle { proc }
    }

    pub fn spawn(self, code: &[u8], args: Vec<Vec<u8>>) -> ProcessHandle {
        let uint8array = js_sys::Uint8Array::new_with_length(code.len() as u32);
        uint8array.copy_from(code);
        let module = Module::new(&uint8array).expect("could not create module from wasm bytes");

        self.spawn_with_module(module, args)
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
