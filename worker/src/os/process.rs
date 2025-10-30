use std::{cell::RefCell, rc::Rc};

use anyhow::{anyhow, Result};
use enum_as_inner::EnumAsInner;
use futures::{
    channel::oneshot::{channel, Receiver, Sender},
    lock::Mutex,
};
use js_sys::{
    Object, Reflect, SharedArrayBuffer,
    WebAssembly::{Memory, Module},
};
use wasm_bindgen::{closure::Closure, JsCast};
use web_sys::{Worker, WorkerOptions, WorkerType};
use web_time::Instant;

use super::{syscall, Fs, Inode, Pipe};

type WriteFn = Rc<dyn Fn(&[u8]) -> usize>;

#[derive(EnumAsInner)]
pub enum FdEntry {
    WriteFn(WriteFn),
    Data {
        data: Vec<u8>,
        offset: usize,
    },
    Dir(Inode),
    /// inode, offset, append
    File(Inode, usize, bool),
    Pipe(Pipe),
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
    pub threads: Vec<Worker>,
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
        for worker in inner.threads.drain(..) {
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
        Reflect::set(&msg, &"module".into(), &self.module).expect("could not set module");
        Reflect::set(&msg, &"memory".into(), &self.memory).expect("could not set memory");
        Reflect::set(&msg, &"channel".into(), &channel).expect("could not set channel");
        if let Some(arg) = arg {
            Reflect::set(&msg, &"tid".into(), &tid.into()).expect("could not set argument");
            Reflect::set(&msg, &"arg".into(), &arg.into()).expect("could not set argument");
        }
        worker
            .post_message(&msg)
            .expect("failed sending init message to worker");

        let proc = Rc::downgrade(self);
        worker.set_onmessage(Some(
            Closure::<dyn Fn(_)>::new(move |msg| {
                if let Some(proc) = proc.upgrade() {
                    syscall::handle_message(proc, channel.clone(), msg);
                }
            })
            .into_js_value()
            .unchecked_ref(),
        ));

        self.inner.borrow_mut().threads.push(worker);

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
    preopen: Option<Vec<Vec<u8>>>,
    stdin: Option<FdEntry>,
    stdout: Option<FdEntry>,
    stderr: Option<FdEntry>,
    args: Vec<Vec<u8>>,
    env: Vec<Vec<u8>>,
    max_memory: Option<u32>,
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

    pub fn arg(mut self, arg: impl Into<Vec<u8>>) -> Self {
        let mut arg: Vec<u8> = arg.into();
        arg.push(0);
        self.args.push(arg);
        self
    }

    pub fn args(self, args: impl IntoIterator<Item = impl Into<Vec<u8>>>) -> Self {
        args.into_iter().fold(self, Builder::arg)
    }

    pub fn env(mut self, env: impl Into<Vec<u8>>) -> Self {
        let mut env: Vec<u8> = env.into();
        env.push(0);
        self.env.push(env);
        self
    }

    pub fn _envs(self, envs: impl IntoIterator<Item = impl Into<Vec<u8>>>) -> Self {
        envs.into_iter().fold(self, Builder::env)
    }

    pub fn _preopens(mut self, dirs: Vec<Vec<u8>>) -> Self {
        self.preopen = Some(dirs);
        self
    }

    /// Set the maximum memory (in pages of 64KiB) for the process.
    pub fn max_memory(mut self, max_memory: Option<u32>) -> Self {
        self.max_memory = max_memory;
        self
    }

    pub fn spawn_with_module(self, module: Module) -> ProcessHandle {
        let imports_memory = Module::imports(&module).iter().any(|import| {
            let kind = Reflect::get(&import, &"kind".into()).expect("could not get import kind");
            let module =
                Reflect::get(&import, &"module".into()).expect("could not get import module");
            let name = Reflect::get(&import, &"name".into()).expect("could not get import name");
            kind.as_string() == Some("memory".to_string())
                && module.as_string() == Some("env".to_string())
                && name.as_string() == Some("memory".to_string())
        });
        assert!(
            imports_memory,
            "module should import memory from env.memory"
        );

        // TODO: get the opts from the module
        let mem_opts = Object::new();
        Reflect::set(&mem_opts, &"initial".into(), &640.into())
            .expect("could not set initial memory size");
        Reflect::set(
            &mem_opts,
            &"maximum".into(),
            &self.max_memory.unwrap_or(65536).into(),
        )
        .expect("could not set maximum memory size");
        Reflect::set(&mem_opts, &"shared".into(), &true.into())
            .expect("could not set shared memory option");
        let memory = Memory::new(&mem_opts).expect("could not create memory");

        let start_instant = Instant::now();

        let fs = self.fs.unwrap_or_default();

        let mut fds = vec![self.stdin, self.stdout, self.stderr];
        if let Some(preopen) = self.preopen {
            for path in preopen {
                let inode = fs.get(fs.root(), &path).unwrap();
                assert!(fs.entries[inode as usize].is_dir());
                fds.push(Some(FdEntry::Dir(inode)));
            }
        } else {
            fds.push(Some(FdEntry::Dir(fs.root())));
        }

        let (termination_send, termination_recv) = channel();

        let inner = ProcessInner {
            fds,
            status_code: StatusCode::Signaled,
            threads: Vec::new(),
            termination_recv,
            fs,
        };

        let proc = Rc::new(Process {
            module,
            memory,
            start_instant,
            args: self.args,
            env: self.env,
            termiation_send: Mutex::new(termination_send),
            inner: RefCell::new(inner),
        });

        proc.spawn_thread(None);

        ProcessHandle { proc }
    }

    pub fn spawn_with_code(self, code: &[u8]) -> ProcessHandle {
        let uint8array = js_sys::Uint8Array::new_with_length(code.len() as u32);
        uint8array.copy_from(code);
        let module = Module::new(&uint8array).expect("could not create module from wasm bytes");
        self.spawn_with_module(module)
    }

    pub fn spawn_with_path(self, path: &[u8]) -> ProcessHandle {
        let code = self.fs.as_ref().unwrap().get_file_with_path(path).unwrap();
        self.spawn_with_code(&code)
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
