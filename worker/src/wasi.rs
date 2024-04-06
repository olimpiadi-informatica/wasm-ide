use core::panic;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::{bail, Context, Result};
use async_channel::{unbounded, Sender};
use js_sys::{Array, WebAssembly};
use tracing::{debug, error, info, instrument, warn};
use wasm_bindgen::JsValue;
use wasmer::{
    vm::VMMemory, Engine, Exports, ExternType, Function, FunctionEnv, FunctionEnvMut, Global,
    Instance, Memory, MemoryAccessError, MemoryType, Module, Store, StoreMut, Value, ValueType,
    WasmPtr,
};

use web_time::Instant;

use crate::{
    compiler::ExecutionOutcome,
    instrument::{instrument_binary, GLOBAL_BLOCKS_BEFORE_TICK},
    thread::spawn,
};

pub type Inode = u64;

#[derive(Clone, Debug)]
enum FsEntry {
    Dir(HashMap<Vec<u8>, Inode>),
    File(Arc<Vec<u8>>),
}

#[derive(Clone)]
pub struct Fs {
    entries: Vec<FsEntry>,
    parent_pointers: Vec<Inode>,
}

#[derive(Debug)]
pub enum FsError {
    NotDir,
    IsDir,
    DoesNotExist,
}

impl Fs {
    pub fn new() -> Fs {
        Fs {
            entries: vec![FsEntry::Dir(HashMap::new())],
            parent_pointers: vec![0],
        }
    }
    pub fn from_files(files: Vec<(Vec<u8>, Arc<Vec<u8>>)>) -> Fs {
        let mut fs = Fs::new();
        for (path, contents) in files {
            let components: Vec<_> = path.split(|x| *x == b'/').map(|x| x.to_vec()).collect();
            let mut cur = fs.root();
            for c in &components[..components.len() - 1] {
                if c.is_empty() {
                    continue;
                }
                let FsEntry::Dir(dir) = &mut fs.entries[cur as usize] else {
                    warn!("invalid file set");
                    panic!("invalid files");
                };
                if let Some(e) = dir.get(c) {
                    cur = *e;
                } else {
                    cur = fs.add_entry(cur, c, FsEntry::Dir(HashMap::new()));
                }
            }
            fs.add_file(cur, &components.last().unwrap(), contents);
        }
        fs
    }
    pub fn root(&self) -> Inode {
        0
    }
    pub fn add_file(&mut self, parent: Inode, name: &[u8], data: Arc<Vec<u8>>) {
        self.add_entry(parent, name, FsEntry::File(data));
    }
    pub fn get(&self, parent: Inode, path: &[u8]) -> Result<Inode, FsError> {
        if path.is_empty() {
            return Ok(parent);
        }
        let FsEntry::Dir(dir) = &self.entries[parent as usize] else {
            return Err(FsError::NotDir);
        };
        let mut path = path.splitn(2, |x| *x == b'/');
        let cur = path.next().unwrap();
        let rest = path.next().unwrap_or(b"");
        if cur == b"." || cur == b"" {
            self.get(parent, rest)
        } else if cur == b".." {
            self.get(self.parent_pointers[parent as usize], rest)
        } else if let Some(child) = dir.get(cur) {
            self.get(*child, rest)
        } else {
            Err(FsError::DoesNotExist)
        }
    }
    pub fn get_file(&self, inode: Inode) -> Result<&Arc<Vec<u8>>, FsError> {
        match &self.entries[inode as usize] {
            FsEntry::Dir(_) => Err(FsError::IsDir),
            FsEntry::File(f) => Ok(f),
        }
    }
    fn add_entry(&mut self, parent: Inode, name: &[u8], entry: FsEntry) -> Inode {
        let new_entry = self.entries.len() as Inode;
        self.entries.push(entry);
        self.parent_pointers.push(parent);
        let FsEntry::Dir(dir) = &mut self.entries[parent as usize] else {
            panic!("invalid call to add_entry");
        };
        dir.insert(name.to_vec(), new_entry);
        new_entry
    }
}

impl Default for Fs {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
enum Fd {
    Stdin(u64),
    Stdout(u64),
    Stderr(u64),
    Dir(Inode),
    File(Inode, u64),
    Closed,
}

pub struct Executable {
    pub exe: Arc<Vec<u8>>,
    pub fs: Fs,
    pub args: Vec<Vec<u8>>,
    pub env: Vec<(Vec<u8>, Vec<u8>)>,
    pub well_known_binary: Option<&'static str>,
}

enum WasiThreadMsg {
    SpawnThread(i32, i32),
    ThreadExit,
}

struct SharedWasiCtx {
    args: Vec<Vec<u8>>,
    env: Vec<Vec<u8>>,
    return_value: Option<u32>,
    kill_message: Option<String>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdin: Arc<Vec<u8>>,
    file_table: Vec<Fd>,
    fs: Fs,
    start: Instant,
    closed_fds: Vec<usize>,
    should_stop: fn() -> bool,
    stream_stdin: Option<fn(&mut [u8]) -> usize>,
    stream_stdout: Option<fn(&[u8])>,
    stream_stderr: Option<fn(&[u8])>,
    total_bytes_written: usize,
    thread_count: usize,
    notify: Option<fn()>,
}

impl SharedWasiCtx {
    fn new(
        args: Vec<Vec<u8>>,
        env: Vec<(Vec<u8>, Vec<u8>)>,
        fs: Fs,
        stdin: Vec<u8>,
        should_stop: fn() -> bool,
        stream_stdin: Option<fn(&mut [u8]) -> usize>,
        stream_stdout: Option<fn(&[u8])>,
        stream_stderr: Option<fn(&[u8])>,
        notify: Option<fn()>,
    ) -> Arc<Mutex<SharedWasiCtx>> {
        let zeroterm = |mut x: Vec<u8>| {
            x.push(0);
            x
        };
        Arc::new(Mutex::new(SharedWasiCtx {
            args: args.into_iter().map(zeroterm).collect(),
            env: env
                .into_iter()
                .map(|mut x| {
                    x.0.push(b'=');
                    x.0.append(&mut x.1);
                    zeroterm(x.0)
                })
                .collect(),
            return_value: None,
            kill_message: None,
            stdout: vec![],
            stderr: vec![],
            file_table: vec![
                Fd::Stdin(0),
                Fd::Stdout(0),
                Fd::Stderr(0),
                Fd::Dir(fs.root()),
            ],
            start: Instant::now(),
            stdin: Arc::new(stdin),
            fs,
            closed_fds: vec![],
            should_stop,
            stream_stdin,
            stream_stdout,
            stream_stderr,
            total_bytes_written: 0,
            thread_count: 0,
            notify,
        }))
    }
}

struct WasiCtx {
    memory: Memory,
    blocks_before_next_tick: Option<Global>,
    msg_sender: Sender<WasiThreadMsg>,
    shared: Arc<Mutex<SharedWasiCtx>>,
    thread_arg: Option<(i32, i32)>,
}

impl WasiCtx {
    fn new(
        memory: Memory,
        msg_sender: Sender<WasiThreadMsg>,
        shared: Arc<Mutex<SharedWasiCtx>>,
        thread_arg: Option<(i32, i32)>,
    ) -> WasiCtx {
        WasiCtx {
            memory,
            blocks_before_next_tick: None,
            msg_sender,
            shared,
            thread_arg,
        }
    }
}

type Errno = u32;

const ERRNO_SUCCESS: Errno = 0;
const ERRNO_BADF: Errno = 8;
const ERRNO_FAULT: Errno = 21;
const ERRNO_INVAL: Errno = 28;
const ERRNO_ISDIR: Errno = 31;
const ERRNO_NOENT: Errno = 44;
const ERRNO_NOTDIR: Errno = 54;
const ERRNO_PERM: Errno = 63;

#[repr(C)]
#[derive(ValueType, Clone, Copy, Debug)]
struct FdStatT {
    fs_filetype: u8,
    fs_flags: u16,
    fs_rights_base: u64,
    fs_rights_inheriting: u64,
}

#[repr(C)]
#[derive(ValueType, Clone, Copy, Debug)]
struct FileStatT {
    dev: u64,
    inode: u64,
    filetype: u8,
    nlink: u64,
    size: u64,
    atim: u64,
    mtim: u64,
    ctim: u64,
}

#[repr(C)]
#[derive(ValueType, Clone, Copy, Debug)]
struct IoVecT {
    buf: WasmPtr<u8>,
    buf_len: u32,
}

#[repr(C)]
#[derive(ValueType, Clone, Copy)]
struct PreStatT {
    tag: u8,
    name_len: u32,
}

fn gettid(env: &FunctionEnvMut<WasiCtx>) -> Option<i32> {
    env.data().thread_arg.map(|x| x.0)
}

fn ea_get(
    mut env: FunctionEnvMut<WasiCtx>,
    argv: WasmPtr<WasmPtr<u8>>,
    argv_buf: WasmPtr<u8>,
    data: Vec<Vec<u8>>,
) -> Errno {
    let (env, store) = env.data_and_store_mut();
    let memory = env.memory.view(&store);
    let mut offs = 0;
    for (i, arg) in data.iter().enumerate() {
        if !argv
            .add_offset(i as u32)
            .map(|x| -> Result<(), MemoryAccessError> {
                Ok(x.write(&memory, argv_buf.add_offset(offs)?)?)
            })
            .is_ok_and(|x| x.is_ok())
        {
            return ERRNO_FAULT;
        }

        debug!("arg: {}", String::from_utf8_lossy(arg));
        if !argv_buf
            .add_offset(offs)
            .map(|x| -> Result<(), MemoryAccessError> {
                Ok(x.slice(&memory, arg.len() as u32)?.write_slice(&arg[..])?)
            })
            .is_ok_and(|x| x.is_ok())
        {
            return ERRNO_FAULT;
        }
        offs += arg.len() as u32;
    }
    ERRNO_SUCCESS
}

fn ea_sizes_get(
    mut env: FunctionEnvMut<WasiCtx>,
    count: WasmPtr<u32>,
    totsize: WasmPtr<u32>,
    data: Vec<Vec<u8>>,
) -> Errno {
    let (env, store) = env.data_and_store_mut();
    let memory = env.memory.view(&store);
    if count.write(&memory, data.len() as u32).is_err() {
        ERRNO_FAULT
    } else if totsize
        .write(&memory, data.iter().map(|x| x.len()).sum::<usize>() as u32)
        .is_err()
    {
        ERRNO_FAULT
    } else {
        ERRNO_SUCCESS
    }
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn args_get(
    env: FunctionEnvMut<WasiCtx>,
    argv: WasmPtr<WasmPtr<u8>>,
    argv_buf: WasmPtr<u8>,
) -> Errno {
    let args = env.data().shared.lock().unwrap().args.clone();
    ea_get(env, argv, argv_buf, args)
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn args_sizes_get(env: FunctionEnvMut<WasiCtx>, argc: WasmPtr<u32>, totarg: WasmPtr<u32>) -> Errno {
    let args = env.data().shared.lock().unwrap().args.clone();
    ea_sizes_get(env, argc, totarg, args)
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn environ_get(
    env: FunctionEnvMut<WasiCtx>,
    envv: WasmPtr<WasmPtr<u8>>,
    env_buf: WasmPtr<u8>,
) -> Errno {
    let envs = env.data().shared.lock().unwrap().env.clone();
    ea_get(env, envv, env_buf, envs)
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn environ_sizes_get(
    env: FunctionEnvMut<WasiCtx>,
    envc_ptr: WasmPtr<u32>,
    totenv_ptr: WasmPtr<u32>,
) -> Errno {
    let envs = env.data().shared.lock().unwrap().env.clone();
    ea_sizes_get(env, envc_ptr, totenv_ptr, envs)
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn clock_res_get(
    mut env: FunctionEnvMut<WasiCtx>,
    _clock_id: u32,
    timestamp: WasmPtr<u64>,
) -> Errno {
    let (env, store) = env.data_and_store_mut();
    let memory = env.memory.view(&store);
    if timestamp.write(&memory, 1).is_err() {
        return ERRNO_FAULT;
    }
    ERRNO_SUCCESS
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn clock_time_get(
    mut env: FunctionEnvMut<WasiCtx>,
    _clock_id: u32,
    _precision: u64,
    time: WasmPtr<u64>,
) -> Errno {
    let (env, store) = env.data_and_store_mut();
    let memory = env.memory.view(&store);
    let mut env = env.shared.lock().unwrap();
    let env = &mut *env;
    let elapsed = env.start.elapsed().as_nanos();
    if time.write(&memory, elapsed as u64).is_err() {
        return ERRNO_FAULT;
    }
    ERRNO_SUCCESS
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn fd_advise(
    _env: FunctionEnvMut<WasiCtx>,
    fd: u32,
    _offset: u64,
    _len: u64,
    _advice: u8,
) -> Errno {
    // Ignored.
    ERRNO_SUCCESS
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn fd_allocate(_env: FunctionEnvMut<WasiCtx>, fd: u32, _offset: u64, _len: u64) -> Errno {
    ERRNO_PERM
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn fd_close(mut env: FunctionEnvMut<WasiCtx>, fd: u32) -> Errno {
    let (env, _) = env.data_and_store_mut();
    let mut env = env.shared.lock().unwrap();
    let env = &mut *env;
    let fd = fd as usize;
    if matches!(env.file_table[fd], Fd::Closed) {
        return ERRNO_BADF;
    }
    env.file_table[fd] = Fd::Closed;
    env.closed_fds.push(fd);
    ERRNO_SUCCESS
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn fd_datasync(_env: FunctionEnvMut<WasiCtx>, fd: u32) -> Errno {
    ERRNO_SUCCESS
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn fd_fdstat_get(mut env: FunctionEnvMut<WasiCtx>, fd: u32, fdstat: WasmPtr<FdStatT>) -> Errno {
    let (env, store) = env.data_and_store_mut();
    let memory = env.memory.view(&store);
    let mut env = env.shared.lock().unwrap();
    let env = &mut *env;
    let file = &env.file_table[fd as usize];
    const READ: u64 = 1 << 1;
    const SEEK: u64 = 1 << 2;
    const WRITE: u64 = 1 << 6;
    const OPEN: u64 = 1 << 12;
    const READDIR: u64 = 1 << 13;
    const FILESTAT_GET: u64 = 1 << 17;
    const FD_FILESTAT_GET: u64 = 1 << 21;
    let lfdstat = match file {
        Fd::Stdout(_) | Fd::Stderr(_) => {
            FdStatT {
                fs_filetype: CHAR,
                fs_flags: 1, // ?
                fs_rights_base: WRITE,
                fs_rights_inheriting: WRITE,
            }
        }
        Fd::Dir(_) => FdStatT {
            fs_filetype: DIR,
            fs_flags: 0,
            fs_rights_base: OPEN | FILESTAT_GET | FD_FILESTAT_GET | READDIR,
            fs_rights_inheriting: READ | OPEN | FILESTAT_GET | FD_FILESTAT_GET | SEEK | READDIR,
        },
        Fd::File(_, _) | Fd::Stdin(_) => FdStatT {
            fs_filetype: DIR,
            fs_flags: 0,
            fs_rights_base: READ | SEEK | FD_FILESTAT_GET,
            fs_rights_inheriting: READ | SEEK | FD_FILESTAT_GET,
        },
        Fd::Closed => {
            return ERRNO_BADF;
        }
    };
    debug!("fd_fdstat_get {fd} {lfdstat:?}");
    if fdstat.write(&memory, lfdstat).is_err() {
        ERRNO_FAULT
    } else {
        ERRNO_SUCCESS
    }
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn fd_fdstat_set_flags(_env: FunctionEnvMut<WasiCtx>, fd: u32, _flags: u16) -> Errno {
    // Noop.
    ERRNO_SUCCESS
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn fd_fdstat_set_rights(
    _env: FunctionEnvMut<WasiCtx>,
    fd: u32,
    _fs_rights_base: u64,
    _fs_rights_inheriting: u64,
) -> Errno {
    // TODO ?
    ERRNO_SUCCESS
}

const FILE: u8 = 4;
const DIR: u8 = 3;
const CHAR: u8 = 2;

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn fd_filestat_get(
    mut env: FunctionEnvMut<WasiCtx>,
    fd: u32,
    filestat: WasmPtr<FileStatT>,
) -> Errno {
    let (env, store) = env.data_and_store_mut();
    let memory = env.memory.view(&store);
    let mut env = env.shared.lock().unwrap();
    let env = &mut *env;
    let Some(fd) = env.file_table.get_mut(fd as usize) else {
        return ERRNO_BADF;
    };
    let mut fstat = FileStatT {
        dev: 0,
        inode: 0,
        filetype: DIR,
        nlink: 1,
        size: 1,
        atim: 0,
        mtim: 0,
        ctim: 0,
    };
    match fd {
        Fd::Dir(inode) => {
            fstat.inode = *inode;
        }
        Fd::Stdin(_) => {
            fstat.dev = 1;
            fstat.filetype = FILE;
            fstat.size = env.stdin.len() as u64;
        }
        Fd::Stdout(_) | Fd::Stderr(_) => {
            fstat.dev = 1;
            fstat.inode = if matches!(fd, Fd::Stdout(_)) { 1 } else { 2 };
            fstat.filetype = CHAR;
        }
        Fd::File(inode, _) => {
            fstat.filetype = FILE;
            fstat.inode = *inode;
            fstat.size = env.fs.get_file(*inode).unwrap().len() as u64;
        }
        Fd::Closed => {
            return ERRNO_BADF;
        }
    }
    debug!("{:?}", fstat);
    if filestat.write(&memory, fstat).is_err() {
        ERRNO_FAULT
    } else {
        ERRNO_SUCCESS
    }
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn fd_filestat_set_size(_env: FunctionEnvMut<WasiCtx>, fd: u32, _size: u64) -> Errno {
    ERRNO_PERM
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn fd_filestat_set_times(
    _env: FunctionEnvMut<WasiCtx>,
    fd: u32,
    _atim: u64,
    _mtim: u64,
    _fst_flags: u16,
) -> Errno {
    ERRNO_PERM
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn fd_pread(
    mut env: FunctionEnvMut<WasiCtx>,
    fd: u32,
    iovs: WasmPtr<IoVecT>,
    iovs_len: u32,
    mut offset: u64,
    nread: WasmPtr<u32>,
) -> Errno {
    let (env, store) = env.data_and_store_mut();
    let memory = env.memory.view(&store);
    let mut env = env.shared.lock().unwrap();
    let env = &mut *env;
    let mut iovecs = vec![
        IoVecT {
            buf: WasmPtr::null(),
            buf_len: 0
        };
        iovs_len as usize
    ];
    if !iovs
        .slice(&memory, iovs_len)
        .map(|x| x.read_slice(&mut iovecs))
        .is_ok_and(|x| x.is_ok())
    {
        return ERRNO_INVAL;
    }
    debug!("pread {fd} {iovecs:?}");
    let (data, is_stdin) = match env.file_table.get_mut(fd as usize).unwrap() {
        Fd::File(inode, _) => (env.fs.get_file(*inode).unwrap().clone(), false),
        Fd::Stdin(_) => (env.stdin.clone(), true),
        _ => return ERRNO_BADF,
    };
    if is_stdin && env.stream_stdin.is_some() {
        return ERRNO_BADF;
    }
    let mut nr = 0;
    for IoVecT { buf, buf_len } in iovecs {
        let len = buf_len.min(data.len().saturating_sub(offset as usize) as u32);
        nr += len;
        let slice = &data[offset as usize..offset as usize + len as usize];
        debug!("{}", String::from_utf8_lossy(slice));
        if !buf
            .slice(&memory, len)
            .map(|x| x.write_slice(slice))
            .is_ok_and(|x| x.is_ok())
        {
            return ERRNO_INVAL;
        }
        offset += len as u64;
    }
    if nread.write(&memory, nr).is_err() {
        return ERRNO_FAULT;
    }
    ERRNO_SUCCESS
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn fd_prestat_get(mut env: FunctionEnvMut<WasiCtx>, fd: u32, prestat: WasmPtr<PreStatT>) -> Errno {
    let (env, store) = env.data_and_store_mut();
    let memory = env.memory.view(&store);
    let mut env = env.shared.lock().unwrap();
    let env = &mut *env;
    let Some(Fd::Dir(inode)) = env.file_table.get(fd as usize) else {
        return ERRNO_BADF;
    };
    if *inode != 0 {
        return ERRNO_BADF;
    }
    let prestat = prestat.deref(&memory);
    if prestat
        .write(PreStatT {
            tag: 0,
            name_len: 1,
        })
        .is_err()
    {
        return ERRNO_FAULT;
    }
    ERRNO_SUCCESS
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn fd_prestat_dir_name(
    mut env: FunctionEnvMut<WasiCtx>,
    fd: u32,
    path_buf: WasmPtr<u8>,
    _len: u32,
) -> Errno {
    let (env, store) = env.data_and_store_mut();
    let memory = env.memory.view(&store);
    let mut env = env.shared.lock().unwrap();
    let env = &mut *env;
    let Some(Fd::Dir(inode)) = env.file_table.get(fd as usize) else {
        return ERRNO_BADF;
    };
    if *inode != 0 {
        return ERRNO_BADF;
    }
    if path_buf.write(&memory, b'/').is_err() {
        return ERRNO_FAULT;
    }
    ERRNO_SUCCESS
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn fd_pwrite(
    _env: FunctionEnvMut<WasiCtx>,
    fd: u32,
    _iovs: WasmPtr<IoVecT>,
    _iovs_len: u32,
    _offset: u64,
    _nwritten: WasmPtr<u32>,
) -> Errno {
    ERRNO_PERM
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn fd_read(
    mut env: FunctionEnvMut<WasiCtx>,
    fd: u32,
    iovs: WasmPtr<IoVecT>,
    iovs_len: u32,
    nread: WasmPtr<u32>,
) -> Errno {
    let (env, store) = env.data_and_store_mut();
    let memory = env.memory.view(&store);
    let mut local_env = env.shared.lock().unwrap();
    let mut nr = 0;
    let mut iovecs = vec![
        IoVecT {
            buf: WasmPtr::null(),
            buf_len: 0
        };
        iovs_len as usize
    ];
    if !iovs
        .slice(&memory, iovs_len)
        .map(|x| x.read_slice(&mut iovecs))
        .is_ok_and(|x| x.is_ok())
    {
        return ERRNO_INVAL;
    }
    debug!("read {fd} {iovecs:?}");
    if matches!(
        local_env.file_table.get_mut(fd as usize).unwrap(),
        Fd::Stdin(_)
    ) && local_env.stream_stdin.is_some()
    {
        let stream = local_env.stream_stdin.unwrap();
        // Drop the lock here, as otherwise this might lock up other threads i.e. in their tick()
        // functions.
        drop(local_env);
        let mut tbuf = vec![];
        let IoVecT { buf, buf_len } = iovecs[0];
        tbuf.resize(buf_len as usize, 0);
        loop {
            check_should_stop(env, &store);
            nr = stream(&mut tbuf);
            if nr != 0 {
                break;
            }
        }
        debug!("{}", String::from_utf8_lossy(&tbuf[..]));
        if !buf
            .slice(&memory, nr as u32)
            .map(|x| x.write_slice(&tbuf[..nr]))
            .is_ok_and(|x| x.is_ok())
        {
            return ERRNO_INVAL;
        }
    } else {
        let env = &mut *local_env;
        let (data, offset) = match env.file_table.get_mut(fd as usize).unwrap() {
            Fd::File(inode, offset) => (env.fs.get_file(*inode).unwrap().clone(), offset),
            Fd::Stdin(offset) => (env.stdin.clone(), offset),
            _ => return ERRNO_BADF,
        };
        for IoVecT { buf, buf_len } in iovecs {
            let len = buf_len.min(data.len().saturating_sub(*offset as usize) as u32);
            nr += len as usize;
            let slice = &data[*offset as usize..*offset as usize + len as usize];
            debug!("{}", String::from_utf8_lossy(slice));
            if !buf
                .slice(&memory, len)
                .map(|x| x.write_slice(slice))
                .is_ok_and(|x| x.is_ok())
            {
                return ERRNO_INVAL;
            }
            *offset += len as u64;
        }
    }
    if nread.write(&memory, nr as u32).is_err() {
        return ERRNO_FAULT;
    }
    ERRNO_SUCCESS
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn fd_readdir(
    mut env: FunctionEnvMut<WasiCtx>,
    fd: u32,
    buf: WasmPtr<u8>,
    buf_len: u32,
    cookie: u64,
    used: WasmPtr<u32>,
) -> Errno {
    let (env, store) = env.data_and_store_mut();
    let memory = env.memory.view(&store);
    let mut env = env.shared.lock().unwrap();
    let env = &mut *env;
    let Some(Fd::Dir(inode)) = env.file_table.get_mut(fd as usize) else {
        return ERRNO_BADF;
    };
    let FsEntry::Dir(entries) = &env.fs.entries[*inode as usize] else {
        return ERRNO_NOTDIR;
    };
    debug!(
        "fd_readdir {fd} {:?}",
        entries
            .keys()
            .map(|x| String::from_utf8_lossy(x))
            .collect::<Vec<_>>()
    );
    let mut tmpbuf = vec![];
    for (num, (name, ino)) in entries.iter().enumerate().skip(cookie as usize) {
        if tmpbuf.len() as u32 >= buf_len {
            break;
        }
        let d_next = (num + 1) as u64;
        tmpbuf.extend_from_slice(&d_next.to_le_bytes());
        tmpbuf.extend_from_slice(&ino.to_le_bytes());
        tmpbuf.extend_from_slice(&(name.len() as u32).to_le_bytes());
        let filetype = if matches!(env.fs.entries[*ino as usize], FsEntry::Dir(_)) {
            DIR
        } else {
            FILE
        };
        tmpbuf.extend_from_slice(&(filetype as u32).to_le_bytes());
        tmpbuf.extend_from_slice(&name[..]);
    }
    let to_copy = buf_len.min(tmpbuf.len() as u32);
    if !buf
        .slice(&memory, to_copy)
        .map(|x| x.write_slice(&tmpbuf[..to_copy as usize]))
        .is_ok_and(|x| x.is_ok())
    {
        return ERRNO_INVAL;
    }
    if !used.write(&memory, to_copy).is_ok() {
        ERRNO_INVAL
    } else {
        ERRNO_SUCCESS
    }
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn fd_renumber(_env: FunctionEnvMut<WasiCtx>, from: u32, to: u32) -> Errno {
    // Maybe?
    ERRNO_PERM
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn fd_seek(
    mut env: FunctionEnvMut<WasiCtx>,
    fd: u32,
    offset: i64,
    whence: u8,
    new_offset: WasmPtr<u64>,
) -> Errno {
    debug!("fd_seek {fd} {offset} {whence}");
    const SEEK_SET: u8 = 0;
    const SEEK_CUR: u8 = 1;
    const SEEK_END: u8 = 2;
    let (env, store) = env.data_and_store_mut();
    let memory = env.memory.view(&store);
    let mut env = env.shared.lock().unwrap();
    let env = &mut *env;
    let Some(fd) = env.file_table.get_mut(fd as usize) else {
        return ERRNO_BADF;
    };
    let mut base_off = match (whence, &fd) {
        (SEEK_CUR, _) => 0,
        (SEEK_SET, _) => 0,
        (SEEK_END, Fd::Stdin(_)) => env.stdin.len() as u64,
        (SEEK_END, Fd::Stdout(_)) => env.stdout.len() as u64,
        (SEEK_END, Fd::Stderr(_)) => env.stderr.len() as u64,
        (SEEK_END, Fd::File(inode, _)) => env.fs.get_file(*inode).unwrap().len() as u64,
        _ => return ERRNO_INVAL,
    };
    let foff = match fd {
        Fd::Stdin(foff) => foff,
        Fd::Stdout(foff) => foff,
        Fd::Stderr(foff) => foff,
        Fd::File(_, foff) => foff,
        _ => return ERRNO_BADF,
    };
    if whence == SEEK_CUR {
        base_off = *foff;
    }
    *foff = base_off.saturating_add_signed(offset);
    debug!("fd_seek to {}", *foff);
    if new_offset.write(&memory, *foff).is_err() {
        ERRNO_FAULT
    } else {
        ERRNO_SUCCESS
    }
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn fd_sync(_env: FunctionEnvMut<WasiCtx>, fd: u32) -> Errno {
    ERRNO_SUCCESS
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn fd_tell(env: FunctionEnvMut<WasiCtx>, fd: u32, offset: WasmPtr<u64>) -> Errno {
    fd_seek(env, fd, 0, 1, offset)
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn fd_write(
    mut env: FunctionEnvMut<WasiCtx>,
    fd: u32,
    iovs: WasmPtr<IoVecT>,
    iovs_len: u32,
    nwritten: WasmPtr<u32>,
) -> Errno {
    let (local_env, store) = env.data_and_store_mut();
    let memory = local_env.memory.view(&store);
    let check_stop = {
        let mut env = local_env.shared.lock().unwrap();
        let env = &mut *env;
        let mut iovecs = vec![
            IoVecT {
                buf: WasmPtr::null(),
                buf_len: 0
            };
            iovs_len as usize
        ];
        if !iovs
            .slice(&memory, iovs_len)
            .map(|x| x.read_slice(&mut iovecs))
            .is_ok_and(|x| x.is_ok())
        {
            return ERRNO_INVAL;
        }
        debug!("write {fd} {iovecs:?}");
        let (data, off, stream_fn) = match env.file_table.get_mut(fd as usize).unwrap() {
            Fd::Stdout(off) => (&mut env.stdout, off, env.stream_stdout),
            Fd::Stderr(off) => (&mut env.stderr, off, env.stream_stderr),
            _ => return ERRNO_BADF,
        };
        let mut nw = 0;
        for IoVecT { buf, buf_len } in iovecs {
            let pos = *off as usize;
            let end = pos + buf_len as usize;
            if data.len() < end {
                data.resize(end, 0);
            }
            nw += buf_len;
            let slice = &mut data[pos..end];
            if !buf
                .slice(&memory, buf_len)
                .map(|x| x.read_slice(slice))
                .is_ok_and(|x| x.is_ok())
            {
                return ERRNO_FAULT;
            }
            debug!("{}", String::from_utf8_lossy(slice));
            *off += buf_len as u64;
        }
        // If we are streaming output, we just use `data` as a temporary buffer.
        if let Some(stream_fn) = stream_fn {
            stream_fn(&data);
            data.clear();
            *off = 0;
        }
        if nwritten.write(&memory, nw).is_err() {
            return ERRNO_FAULT;
        }
        env.total_bytes_written += nw as usize;
        const SHOULD_STOP_BYTES: usize = 10_000;
        if env.total_bytes_written > SHOULD_STOP_BYTES {
            env.total_bytes_written = 0;
            true
        } else {
            false
        }
    };
    if check_stop {
        check_should_stop(local_env, &store);
    }
    ERRNO_SUCCESS
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn path_create_directory(
    _env: FunctionEnvMut<WasiCtx>,
    dirfd: u32,
    _path: WasmPtr<u8>,
    _path_len: u32,
) -> Errno {
    ERRNO_PERM
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn path_filestat_get(
    mut env: FunctionEnvMut<WasiCtx>,
    dirfd: u32,
    _flags: u32,
    path: WasmPtr<u8>,
    path_len: u32,
    filestat: WasmPtr<FileStatT>,
) -> Errno {
    debug!("path_filestat_get {dirfd}");
    let (env, store) = env.data_and_store_mut();
    let memory = env.memory.view(&store);
    let mut env = env.shared.lock().unwrap();
    let env = &mut *env;
    let Some(Fd::Dir(dir)) = env.file_table.get_mut(dirfd as usize) else {
        return ERRNO_BADF;
    };
    let mut pathbuf = vec![0; path_len as usize];
    if !path
        .slice(&memory, path_len)
        .map(|x| x.read_slice(&mut pathbuf))
        .is_ok_and(|x| x.is_ok())
    {
        return ERRNO_FAULT;
    }
    debug!(
        "path_filestat_get {dirfd} {}",
        String::from_utf8_lossy(&pathbuf)
    );
    let Ok(inode) = env.fs.get(*dir, &pathbuf) else {
        return ERRNO_NOENT;
    };
    let mut fstat = FileStatT {
        dev: 0,
        inode,
        filetype: DIR,
        nlink: 1,
        size: 1,
        atim: 0,
        mtim: 0,
        ctim: 0,
    };
    match &env.fs.entries[inode as usize] {
        FsEntry::Dir(_) => {}
        FsEntry::File(data) => {
            fstat.size = data.len() as u64;
            fstat.filetype = FILE;
        }
    }
    debug!(
        "path_filestat_get {dirfd} {} {:?}",
        String::from_utf8_lossy(&pathbuf),
        fstat
    );
    if filestat.write(&memory, fstat).is_err() {
        ERRNO_FAULT
    } else {
        ERRNO_SUCCESS
    }
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn path_filestat_set_times(
    _env: FunctionEnvMut<WasiCtx>,
    dirfd: u32,
    _flags: u32,
    _path: WasmPtr<u8>,
    _path_len: u32,
    _filestat: WasmPtr<FileStatT>,
    _atim: u64,
    _mtim: u64,
    _fst_flags: u16,
) -> Errno {
    debug!("path_filestat_set_times {dirfd}");
    ERRNO_PERM
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn path_link(
    _env: FunctionEnvMut<WasiCtx>,
    _old_fd: u32,
    _old_flags: u32,
    _old_path: WasmPtr<u8>,
    _old_path_len: u32,
    _new_fd: u32,
    _new_path: WasmPtr<u8>,
    _new_path_len: u32,
) -> Errno {
    debug!("path_link");
    ERRNO_PERM
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn path_open(
    mut env: FunctionEnvMut<WasiCtx>,
    dirfd: u32,
    _dirflags: u32,
    path_ptr: WasmPtr<u8>,
    path_len: u32,
    o_flags: u16,
    _fs_rights_base: u64,
    _fs_rights_inheriting: u64,
    _fd_flags: u16,
    fd: WasmPtr<u32>,
) -> Errno {
    let (env, store) = env.data_and_store_mut();
    let memory = env.memory.view(&store);
    let mut env = env.shared.lock().unwrap();
    let env = &mut *env;
    let mut path = vec![0; path_len as usize];
    if !path_ptr
        .slice(&memory, path_len)
        .map(|x| x.read_slice(&mut path))
        .is_ok_and(|x| x.is_ok())
    {
        return ERRNO_FAULT;
    }
    debug!("path_open {dirfd} {}", String::from_utf8_lossy(&path));
    let Some(Fd::Dir(inode)) = env.file_table.get(dirfd as usize) else {
        return ERRNO_BADF;
    };
    let new_inode = match env.fs.get(*inode, &path) {
        Ok(inode) => inode,
        Err(FsError::DoesNotExist) => return ERRNO_NOENT,
        Err(FsError::NotDir) => return ERRNO_NOTDIR,
        Err(FsError::IsDir) => return ERRNO_ISDIR,
    };
    if o_flags & 2 != 0 && !matches!(env.fs.entries[new_inode as usize], FsEntry::Dir(_)) {
        return ERRNO_NOTDIR;
    };
    let new_fd = if let Some(f) = env.closed_fds.pop() {
        f
    } else {
        env.file_table.push(Fd::Closed);
        env.file_table.len() - 1
    };
    match env.fs.entries[new_inode as usize] {
        FsEntry::Dir(_) => env.file_table[new_fd] = Fd::Dir(new_inode),
        FsEntry::File(_) => env.file_table[new_fd] = Fd::File(new_inode, 0),
    };
    if fd.write(&memory, new_fd as u32).is_err() {
        return ERRNO_FAULT;
    }
    debug!(
        "path_open {dirfd} {} {new_inode} {new_fd}",
        String::from_utf8_lossy(&path)
    );
    ERRNO_SUCCESS
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn path_readlink(
    _env: FunctionEnvMut<WasiCtx>,
    dirfd: u32,
    _path: WasmPtr<u8>,
    _path_len: u32,
    _buf: WasmPtr<u8>,
    _buf_len: u32,
    _buf_used: WasmPtr<u32>,
) -> Errno {
    debug!("path_readlink {dirfd}");
    ERRNO_PERM
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn path_remove_directory(
    _env: FunctionEnvMut<WasiCtx>,
    dirfd: u32,
    _path: WasmPtr<u8>,
    _path_len: u32,
) -> Errno {
    ERRNO_PERM
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn path_rename(
    _env: FunctionEnvMut<WasiCtx>,
    _old_fd: u32,
    _old_path: WasmPtr<u8>,
    _old_path_len: u32,
    _new_fd: u32,
    _new_path: WasmPtr<u8>,
    _new_path_len: u32,
) -> Errno {
    ERRNO_PERM
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn path_symlink(
    _env: FunctionEnvMut<WasiCtx>,
    _old_fd: u32,
    _old_path: WasmPtr<u8>,
    _old_path_len: u32,
    _fd: u32,
    _new_path: WasmPtr<u8>,
    _new_path_len: u32,
) -> Errno {
    ERRNO_PERM
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn path_unlink_file(
    _env: FunctionEnvMut<WasiCtx>,
    _dirfd: u32,
    _path: WasmPtr<u8>,
    _path_len: u32,
) -> Errno {
    ERRNO_PERM
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn proc_exit(mut env: FunctionEnvMut<WasiCtx>, exitcode: u32) {
    env.data_mut().shared.lock().unwrap().return_value = Some(exitcode.min(128));
    if exitcode != 0 {
        panic!("non-zero exit code");
    }
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn proc_raise(mut env: FunctionEnvMut<WasiCtx>, sig: u8) -> Errno {
    env.data_mut().shared.lock().unwrap().return_value = Some(sig as u32 + 128);
    ERRNO_SUCCESS
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn random_get(mut env: FunctionEnvMut<WasiCtx>, buf: WasmPtr<u8>, buf_len: u32) -> Errno {
    let (env, store) = env.data_and_store_mut();
    let memory = env.memory.view(&store);
    if let Ok(slice) = buf.slice(&memory, buf_len) {
        let mut rand = vec![0; slice.len() as usize];
        let mut seed = 0x78781791u32;
        for v in rand.iter_mut() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            *v = (seed >> 24) as u8;
        }
        slice.write_slice(&rand).unwrap();
    } else {
        return ERRNO_FAULT;
    }
    ERRNO_SUCCESS
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "trace")]
fn sched_yield(_env: FunctionEnvMut<WasiCtx>) -> Errno {
    ERRNO_SUCCESS
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn sock_accept(
    _env: FunctionEnvMut<WasiCtx>,
    _fd: u32,
    _flags: u16,
    _fd_out: WasmPtr<u32>,
) -> Errno {
    ERRNO_PERM
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn sock_recv(
    _env: FunctionEnvMut<WasiCtx>,
    _fd: u32,
    _data: WasmPtr<IoVecT>,
    _data_len: u32,
    _ri_flags: u16,
) -> Errno {
    ERRNO_PERM
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn sock_send(
    _env: FunctionEnvMut<WasiCtx>,
    _fd: u32,
    _data: WasmPtr<IoVecT>,
    _data_len: u32,
    _si_flags: u16,
) -> Errno {
    ERRNO_PERM
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn sock_shutdown(_env: FunctionEnvMut<WasiCtx>, _fd: u32, _sd_flags: u8) -> Errno {
    ERRNO_PERM
}

#[instrument(skip(_env), fields(tid = gettid(&_env)), ret, level = "debug")]
fn poll_oneoff(
    _env: FunctionEnvMut<WasiCtx>,
    _in: WasmPtr<u8>,  // placeholder type
    _out: WasmPtr<u8>, // placeholder type
    _nsubscriptions: u32,
    _nev: u32,
) -> Errno {
    ERRNO_PERM
}

fn check_should_stop(env: &mut WasiCtx, store: &StoreMut) {
    let panic_msg = {
        let should_stop = env.shared.lock().unwrap().should_stop;
        let should_stop = should_stop();
        let mut shared = env.shared.lock().unwrap();
        if shared.return_value.is_some() {
            Some("exit was called")
        } else if shared.kill_message.is_some() {
            warn!(
                "thread {:?} dying: {:?}",
                env.thread_arg, shared.kill_message
            );
            Some("propagating failure")
        } else if env.memory.view(store).data_size() >= 30 * 1024 * 1024 * 1024 / 8 {
            shared.kill_message = Some("Memory limit (3.75GB) exceeded".to_string());
            Some("execution killed")
        } else if should_stop {
            shared.kill_message = Some("Execution killed by user".to_string());
            Some("execution killed")
        } else {
            None
        }
    };
    if let Some(msg) = panic_msg {
        if let Some(notify) = env.shared.lock().unwrap().notify {
            notify();
        }
        panic!("{}", msg);
    }
}

fn tick_fn(mut env: FunctionEnvMut<WasiCtx>) {
    let (env, mut store) = env.data_and_store_mut();
    check_should_stop(env, &store);
    const BLOCK_TICK_INTERVAL: i32 = 1_000_000;
    // See comment in check_should_stop for the rationale behind not unwrap()-ing here.
    if let Some(var) = env.blocks_before_next_tick.as_ref() {
        var.set(&mut store, wasmer::Value::I32(BLOCK_TICK_INTERVAL))
            .unwrap();
    }
}

#[instrument(skip(env), fields(tid = gettid(&env)), ret, level = "debug")]
fn wasi_thread_spawn(mut env: FunctionEnvMut<WasiCtx>, thread_arg: i32) -> i32 {
    const MAX_THREADS: usize = 1 << 24;
    // should call wasi_thread_start on another thread with thread_arg as an argument.
    // Returns a negative number for errors, a non-negative thread id otherwise.
    let (env, store) = env.data_and_store_mut();
    check_should_stop(env, &store);
    let thread_id = {
        let mut shared = env.shared.lock().unwrap();
        shared.thread_count += 1;
        if shared.thread_count >= MAX_THREADS {
            return -1;
        }
        shared.thread_count as i32
    };
    if let Err(e) = env
        .msg_sender
        .try_send(WasiThreadMsg::SpawnThread(thread_id, thread_arg))
    {
        error!("trying to spawn a thread from a terminating process? {e}");
        return -1;
    }
    thread_id
}

macro_rules! wasi_imports {
    ( $store:ident, $wasi_ctx:ident, $( $fun:ident ),* $(,)? ) => {
        {
            let mut import_object = wasmer::Imports::new();
            let mut namespace = wasmer::Exports::new();
            $(
                namespace.insert(
                    stringify!($fun),
                    Function::new_typed_with_env($store, &$wasi_ctx, $fun)
                );
            )*
            import_object.register_namespace("wasi_snapshot_preview1", namespace);
            import_object
        }
    };
}

fn make_imports(
    memory: Memory,
    wasi_ctx: &FunctionEnv<WasiCtx>,
    store: &mut Store,
) -> wasmer::Imports {
    let mut import_object = wasi_imports! {
        store, wasi_ctx,
        args_get, args_sizes_get,
        environ_get, environ_sizes_get,
        clock_res_get, clock_time_get,
        fd_advise, fd_allocate, fd_close, fd_datasync, fd_fdstat_get,
        fd_fdstat_set_flags, fd_fdstat_set_rights, fd_filestat_get,
        fd_filestat_set_size, fd_filestat_set_times, fd_pread,
        fd_prestat_get, fd_prestat_dir_name, fd_pwrite,
        fd_read, fd_readdir, fd_renumber, fd_seek, fd_sync,
        fd_tell, fd_write, path_create_directory, path_filestat_get,
        path_filestat_set_times, path_link, path_open, path_readlink,
        path_remove_directory, path_rename, path_symlink, path_unlink_file,
        proc_exit, proc_raise, random_get, sched_yield, sock_accept, sock_recv,
        sock_send, sock_shutdown, poll_oneoff
    };
    let mut env_module = Exports::new();
    env_module.insert("memory", memory);
    import_object.register_namespace("env", env_module);
    let mut tick_module = Exports::new();
    tick_module.insert(
        crate::instrument::TICK_FN,
        Function::new_typed_with_env(store, &wasi_ctx, tick_fn),
    );
    import_object.register_namespace(crate::instrument::MODULE, tick_module);
    let mut wasi_module = Exports::new();
    wasi_module.insert(
        "thread-spawn",
        Function::new_typed_with_env(store, &wasi_ctx, wasi_thread_spawn),
    );
    import_object.register_namespace("wasi", wasi_module);
    import_object
}

#[instrument(skip(sender, exe, shared_ctx), ret, level = "debug")]
async fn wasi_start_thread(
    msg_arg: JsValue,
    mem_type: MemoryType,
    exe: Arc<Vec<u8>>,
    sender: Sender<WasiThreadMsg>,
    shared_ctx: Arc<Mutex<SharedWasiCtx>>,
    arg: Option<(i32, i32)>,
) {
    debug!("wasi start thread {:?}", arg);
    let thread_fn = move |msg_arg: JsValue| {
        debug!("starting thread: {:?}", arg);
        let sender_copy = sender.clone();
        'run: {
            let array: Array = msg_arg.try_into().expect("unexpected arg");
            let module: WebAssembly::Module = array.get(0).try_into().unwrap();
            let memory: WebAssembly::Memory = array.get(1).try_into().unwrap();
            let engine = Engine::default();
            // We need the `exe` argument as otherwise Wasmer gets confused about the types of the
            // exports in this module.
            let module: Module = (module, &exe[..]).into();
            let mut store = Store::new(engine);
            let memory = VMMemory::new(memory, mem_type);
            let memory = Memory::new_from_existing(&mut store, memory);
            let ctx = WasiCtx::new(memory.clone(), sender, shared_ctx, arg.clone());
            let ctx = FunctionEnv::new(&mut store, ctx);
            let imports = make_imports(memory, &ctx, &mut store);
            debug!("wasi start thread {:?}: imports set up", arg);
            let instance = match Instance::new(&mut store, &module, &imports) {
                Ok(instance) => instance,
                Err(e) => {
                    error!("Instantiation error: {e}");
                    break 'run;
                }
            };
            debug!("wasi start thread {:?}: instance ready", arg);

            ctx.as_mut(&mut store).blocks_before_next_tick = Some(
                instance
                    .exports
                    .get_global(GLOBAL_BLOCKS_BEFORE_TICK)
                    .context("Missing exported tick global")
                    .unwrap()
                    .clone(),
            );
            let res: Result<_> = match arg {
                Some((tid, arg)) => instance
                    .exports
                    .get_function("wasi_thread_start")
                    .map_err(|x| x.into())
                    .and_then(|startfun| {
                        Ok(startfun.call(&mut store, &[Value::I32(tid), Value::I32(arg)])?)
                    }),
                None => instance
                    .exports
                    .get_function("_start")
                    .map_err(|x| x.into())
                    .and_then(|startfun| Ok(startfun.call(&mut store, &[])?)),
            };
            debug!("wasi start thread {:?}: exiting", arg);
            if let Err(e) = res {
                let mut shared = ctx.as_mut(&mut store).shared.lock().unwrap();
                if shared.kill_message.is_none() {
                    warn!("runtime error on thread {:?}: {e}", arg.map(|x| x.0));
                    shared.kill_message = Some(format!("Runtime error: {e}"));
                    if let Some(notify) = shared.notify {
                        notify();
                    }
                }
            }
        };
        let _ = sender_copy.try_send(WasiThreadMsg::ThreadExit);
    };

    spawn(thread_fn, msg_arg).await;
}

impl Executable {
    fn make_module_and_shared_ctx(
        &self,
        store: &mut Store,
        stdin: Vec<u8>,
        should_stop: fn() -> bool,
        stream_stdin: Option<fn(&mut [u8]) -> usize>,
        stream_stdout: Option<fn(&[u8])>,
        stream_stderr: Option<fn(&[u8])>,
        notify: Option<fn()>,
    ) -> Result<(
        (WebAssembly::Module, Arc<Vec<u8>>),
        (WebAssembly::Memory, MemoryType),
        Arc<Mutex<SharedWasiCtx>>,
    )> {
        // Add instrumentation to the module.
        let exe = instrument_binary(&self.exe[..], self.well_known_binary.clone())
            .context("could not add time instrumentation")?;
        let module = wasmer::Module::new(store, &exe[..]).context("Could not create module")?;
        // Create the shared memory.
        let memory = module
            .imports()
            .find_map(
                |import| match (import.ty(), import.module(), import.name()) {
                    (ExternType::Memory(m), "env", "memory") => Some(m.clone()),
                    _ => None,
                },
            )
            .expect("no imported memory");
        let mem = Memory::new(store, memory).unwrap();
        // Create the Wasi context + the imports.
        let ctx = SharedWasiCtx::new(
            self.args.clone(),
            self.env.clone(),
            self.fs.clone(),
            stdin,
            should_stop,
            stream_stdin,
            stream_stdout,
            stream_stderr,
            notify,
        );
        let mem_info: (JsValue, _) = mem.try_clone(store).unwrap().into();
        Ok((
            (module.into(), Arc::new(exe)),
            (mem_info.0.try_into().unwrap(), mem_info.1),
            ctx,
        ))
    }

    pub async fn run(
        &self,
        input: Vec<u8>,
        should_stop: fn() -> bool,
        stream_stdin: Option<fn(&mut [u8]) -> usize>,
        stream_stdout: Option<fn(&[u8])>,
        stream_stderr: Option<fn(&[u8])>,
        notify: Option<fn()>,
    ) -> Result<ExecutionOutcome> {
        let mut store = Store::default();
        let ((module, exe), (js_memory, memory_type), shared_ctx) = self
            .make_module_and_shared_ctx(
                &mut store,
                input,
                should_stop,
                stream_stdin,
                stream_stdout,
                stream_stderr,
                notify,
            )?;
        let wasi_init_msg = Array::new();
        wasi_init_msg.push(&module);
        wasi_init_msg.push(&js_memory);
        let (sender, receiver) = unbounded();
        wasi_start_thread(
            wasi_init_msg.clone().into(),
            memory_type,
            exe.clone(),
            sender.clone(),
            shared_ctx.clone(),
            None,
        )
        .await;

        while receiver.sender_count() > 1 {
            let msg = receiver.recv().await.unwrap();
            match msg {
                WasiThreadMsg::ThreadExit => continue,
                WasiThreadMsg::SpawnThread(tid, arg) => {
                    wasi_start_thread(
                        wasi_init_msg.clone().into(),
                        memory_type,
                        exe.clone(),
                        sender.clone(),
                        shared_ctx.clone(),
                        Some((tid, arg)),
                    )
                    .await;
                }
            }
        }

        drop(receiver);

        let mut ctx = shared_ctx.lock().unwrap();
        if let Some(rv) = ctx.return_value {
            if rv != 0 {
                bail!("Execution failed because of non-zero return code {rv}.",);
            }
        }
        if let Some(kill_message) = &ctx.kill_message {
            if ctx.return_value != Some(0) {
                bail!("Execution killed: {}.", kill_message,);
            }
        }
        Ok(ExecutionOutcome {
            stdout: std::mem::take(&mut ctx.stdout),
            stderr: std::mem::take(&mut ctx.stderr),
        })
    }
}
