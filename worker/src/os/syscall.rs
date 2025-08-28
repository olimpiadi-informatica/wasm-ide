use std::rc::Rc;

use bitflags::bitflags;
use js_sys::{Atomics, Int32Array, Uint8Array};
use serde::Deserialize;
use serde_repr::Deserialize_repr;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::spawn_local;
use web_sys::MessageEvent;
use zerocopy::{FromBytes, Immutable, IntoBytes};

use crate::os::{FsEntry, FsError, ProcessInner};

use super::{FdEntry, Process, StatusCode};

type Addr = u32;
type Size = u32;
type FileSize = u64;
type Timestamp = u64;
type Fd = u32;
type Advice = u8;
type FstFlags = u16;
type FileDelta = i64;
type ExitCode = u32;
type Signal = u8;
type LookupFlags = u32;

#[repr(u8)]
#[derive(Debug, Clone, Copy, Deserialize_repr, PartialEq, Eq)]
enum Whence {
    Set = 0,
    Cur = 1,
    End = 2,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, Deserialize_repr)]
enum ClockId {
    Monotonic = 0,
    Realtime = 1,
    ProcessCpu = 2,
    ThreadCpu = 3,
}

#[derive(Debug, Clone, Copy, Immutable, IntoBytes, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
struct Rights(u64);

bitflags! {
    impl Rights: u64 {
        const FD_DATASYNC = 1 << 0;
        const FD_READ = 1 << 1;
        const FD_SEEK = 1 << 2;
        const FD_FDSTAT_SET_FLAGS = 1 << 3;
        const FD_SYNC = 1 << 4;
        const FD_TELL = 1 << 5;
        const FD_WRITE = 1 << 6;
        const FD_ADVISE = 1 << 7;
        const FD_ALLOCATE = 1 << 8;
        const PATH_CREATE_DIRECTORY = 1 << 9;
        const PATH_CREATE_FILE = 1 << 10;
        const PATH_LINK_SOURCE = 1 << 11;
        const PATH_LINK_TARGET = 1 << 12;
        const PATH_OPEN = 1 << 13;
        const PATH_READDIR = 1 << 14;
        const PATH_READLINK = 1 << 15;
        const PATH_RENAME_SOURCE = 1 << 16;
        const PATH_RENAME_TARGET = 1 << 17;
        const PATH_FILESTAT_GET = 1 << 18;
        const PATH_FILESTAT_SET_SIZE = 1 << 19;
        const PATH_FILESTAT_SET_TIMES = 1 << 20;
        const FD_FILESTAT_GET = 1 << 21;
        const FD_FILESTAT_SET_SIZE = 1 << 22;
        const FD_FILESTAT_SET_TIMES = 1 << 23;
        const PATH_SYMLINK = 1 << 24;
        const PATH_REMOVE_DIRECTORY = 1 << 25;
        const PATH_UNLINK_FILE = 1 << 26;
        const POLL_FD_READWRITE = 1 << 27;
        const SOCK_SHUTDOWN = 1 << 28;
        const SOCK_ACCEPT = 1 << 29;
    }
}

#[derive(Debug, Clone, Copy, Immutable, IntoBytes, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
struct FdFlags(u16);

bitflags! {
    impl FdFlags: u16 {
        const APPEND = 1 << 0;
        const DSYNC = 1 << 1;
        const NONBLOCK = 1 << 2;
        const RSYNC = 1 << 3;
        const SYNC = 1 << 4;
    }
}

#[derive(Debug, Clone, Copy, Immutable, IntoBytes, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
struct OFlags(u16);

bitflags! {
    impl OFlags: u16 {
        const CREAT = 1 << 0;
        const DIRECTORY = 1 << 1;
        const EXCL = 1 << 2;
        const TRUNC = 1 << 3;
    }
}

#[repr(u16)]
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
#[non_exhaustive]
enum Errno {
    Success = 0,
    Acces = 2,
    Badf = 8,
    Exist = 20,
    Fault = 21,
    Inval = 28,
    IsDir = 31,
    NoEnt = 44,
    NotDir = 54,
    NotSock = 57,
    Perm = 63,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, Immutable, IntoBytes)]
#[allow(dead_code)]
enum FileType {
    Unknown = 0,
    BlockDevice = 1,
    CharacterDevice = 2,
    Directory = 3,
    RegularFile = 4,
    SocketDgram = 5,
    SocketStream = 6,
    SymbolicLink = 7,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ProcMsg {
    Syscall { kind: String, args: Vec<i64> },
    RuntimeError { re: String },
}

pub fn handle_message(proc: Rc<Process>, tid: u32, msg: JsValue) {
    let msg = msg
        .dyn_into::<MessageEvent>()
        .expect("message event expected")
        .data();
    let msg = serde_wasm_bindgen::from_value::<ProcMsg>(msg)
        .expect("failed to deserialize WASI syscall message");

    spawn_local(async move {
        let ret = handle_message_inner(&proc, msg).await;
        match ret {
            Some(Some(ret)) => {
                let channel = proc.inner.borrow().threads[tid as usize - 1].1.clone();
                let array = Int32Array::new(&channel);
                Atomics::store(&array, 0, ret).expect("failed to store result in channel");
                Atomics::notify(&array, 0).expect("failed to notify main thread about result");
            }
            Some(None) => {}
            None => {
                proc.kill(StatusCode::RuntimeError("invalid syscall".into()));
            }
        }
    });
}

async fn handle_message_inner(proc: &Rc<Process>, msg: ProcMsg) -> Option<Option<i32>> {
    let (kind, args) = match msg {
        ProcMsg::Syscall { kind, args } => (kind, args),
        ProcMsg::RuntimeError { re } => {
            proc.kill(StatusCode::RuntimeError(re));
            return Some(None);
        }
    };

    trait Arg<S> {
        fn a(self) -> Option<S>;
    }

    macro_rules! impl_arg_for_int {
        ($($t:ty),*) => {
            $(
                impl Arg<$t> for i64 {
                    fn a(self) -> Option<$t> {
                        let bytes = self.to_le_bytes();
                        let size = std::mem::size_of::<$t>();
                        let x = <$t>::from_le_bytes(bytes[..size].try_into().unwrap());
                        if bytes[size..].iter().any(|&b| b != (if self < 0 { 0xFF } else { 0 })) {
                            return None;
                        }
                        Some(x)
                    }
                }
            )*
        };
    }

    impl_arg_for_int!(i8, i16, i32, i64, u8, u16, u32, u64);

    impl Arg<ClockId> for i64 {
        fn a(self) -> Option<ClockId> {
            match self {
                0 => Some(ClockId::Monotonic),
                1 => Some(ClockId::Realtime),
                2 => Some(ClockId::ProcessCpu),
                3 => Some(ClockId::ThreadCpu),
                _ => None,
            }
        }
    }

    impl Arg<FdFlags> for i64 {
        fn a(self) -> Option<FdFlags> {
            Some(FdFlags(self.a()?))
        }
    }

    impl Arg<Rights> for i64 {
        fn a(self) -> Option<Rights> {
            Some(Rights(self.a()?))
        }
    }

    impl Arg<Whence> for i64 {
        fn a(self) -> Option<Whence> {
            match self {
                0 => Some(Whence::Set),
                1 => Some(Whence::Cur),
                2 => Some(Whence::End),
                _ => None,
            }
        }
    }

    impl Arg<OFlags> for i64 {
        fn a(self) -> Option<OFlags> {
            Some(OFlags(self.a()?))
        }
    }

    Some(Some(match (kind.as_str(), args.as_slice()) {
        ("args_get", &[a, b]) => args_get(proc, a.a()?, b.a()?) as _,
        ("args_sizes_get", &[a, b]) => args_sizes_get(proc, a.a()?, b.a()?) as _,
        ("environ_get", &[a, b]) => environ_get(proc, a.a()?, b.a()?) as _,
        ("environ_sizes_get", &[a, b]) => environ_sizes_get(proc, a.a()?, b.a()?) as _,
        ("clock_res_get", &[a, b]) => clock_res_get(proc, a.a()?, b.a()?) as _,
        ("clock_time_get", &[a, b, c]) => clock_time_get(proc, a.a()?, b.a()?, c.a()?) as _,
        ("fd_advise", &[a, b, c, d]) => fd_advise(proc, a.a()?, b.a()?, c.a()?, d.a()?) as _,
        ("fd_allocate", &[a, b, c]) => fd_allocate(proc, a.a()?, b.a()?, c.a()?) as _,
        ("fd_close", &[a]) => fd_close(proc, a.a()?) as _,
        ("fd_datasync", &[a]) => fd_datasync(proc, a.a()?) as _,
        ("fd_fdstat_get", &[a, b]) => fd_fdstat_get(proc, a.a()?, b.a()?) as _,
        ("fd_fdstat_set_flags", &[a, b]) => fd_fdstat_set_flags(proc, a.a()?, b.a()?) as _,
        ("fd_fdstat_set_rights", &[a, b, c]) => {
            fd_fdstat_set_rights(proc, a.a()?, b.a()?, c.a()?) as _
        }
        ("fd_filestat_get", &[a, b]) => fd_filestat_get(proc, a.a()?, b.a()?) as _,
        ("fd_filestat_set_size", &[a, b]) => fd_filestat_set_size(proc, a.a()?, b.a()?) as _,
        ("fd_filestat_set_times", &[a, b, c, d]) => {
            fd_filestat_set_times(proc, a.a()?, b.a()?, c.a()?, d.a()?) as _
        }
        ("fd_pread", &[a, b, c, d, e]) => {
            fd_pread(proc, a.a()?, b.a()?, c.a()?, d.a()?, e.a()?) as _
        }
        ("fd_prestat_get", &[a, b]) => fd_prestat_get(proc, a.a()?, b.a()?) as _,
        ("fd_prestat_dir_name", &[a, b, c]) => {
            fd_prestat_dir_name(proc, a.a()?, b.a()?, c.a()?) as _
        }
        ("fd_pwrite", &[a, b, c, d, e]) => {
            fd_pwrite(proc, a.a()?, b.a()?, c.a()?, d.a()?, e.a()?) as _
        }
        ("fd_read", &[a, b, c, d]) => fd_read(proc, a.a()?, b.a()?, c.a()?, d.a()?).await as _,
        ("fd_readdir", &[a, b, c, d, e]) => {
            fd_readdir(proc, a.a()?, b.a()?, c.a()?, d.a()?, e.a()?) as _
        }
        ("fd_renumber", &[a, b]) => fd_renumber(proc, a.a()?, b.a()?) as _,
        ("fd_seek", &[a, b, c, d]) => fd_seek(proc, a.a()?, b.a()?, c.a()?, d.a()?) as _,
        ("fd_sync", &[a]) => fd_sync(proc, a.a()?) as _,
        ("fd_tell", &[a, b]) => fd_tell(proc, a.a()?, b.a()?) as _,
        ("fd_write", &[a, b, c, d]) => fd_write(proc, a.a()?, b.a()?, c.a()?, d.a()?) as _,
        ("path_create_directory", &[a, b, c]) => {
            path_create_directory(proc, a.a()?, b.a()?, c.a()?) as _
        }
        ("path_filestat_get", &[a, b, c, d, e]) => {
            path_filestat_get(proc, a.a()?, b.a()?, c.a()?, d.a()?, e.a()?) as _
        }
        ("path_filestat_set_times", &[a, b, c, d, e, f, g]) => {
            path_filestat_set_times(proc, a.a()?, b.a()?, c.a()?, d.a()?, e.a()?, f.a()?, g.a()?)
                as _
        }
        ("path_link", &[a, b, c, d, e, f]) => {
            path_link(proc, a.a()?, b.a()?, c.a()?, d.a()?, e.a()?, f.a()?) as _
        }
        ("path_open", &[a, b, c, d, e, f, g, h, i]) => path_open(
            proc,
            a.a()?,
            b.a()?,
            c.a()?,
            d.a()?,
            e.a()?,
            f.a()?,
            g.a()?,
            h.a()?,
            i.a()?,
        ) as _,
        ("path_readlink", &[a, b, c, d, e, f]) => {
            path_readlink(proc, a.a()?, b.a()?, c.a()?, d.a()?, e.a()?, f.a()?) as _
        }
        ("path_remove_directory", &[a, b, c]) => {
            path_remove_directory(proc, a.a()?, b.a()?, c.a()?) as _
        }
        ("path_rename", &[a, b, c, d, e, f]) => {
            path_rename(proc, a.a()?, b.a()?, c.a()?, d.a()?, e.a()?, f.a()?) as _
        }
        ("path_symlink", &[a, b, c, d, e]) => {
            path_symlink(proc, a.a()?, b.a()?, c.a()?, d.a()?, e.a()?) as _
        }
        ("path_unlink_file", &[a, b, c]) => path_unlink_file(proc, a.a()?, b.a()?, c.a()?) as _,
        ("proc_exit", &[a]) => {
            proc_exit(proc, a.a()?);
            return Some(None);
        }
        ("proc_raise", &[a]) => proc_raise(proc, a.a()?) as _,
        ("sched_yield", &[]) => sched_yield(proc) as _,
        ("random_get", &[a, b]) => random_get(proc, a.a()?, b.a()?) as _,
        ("sock_accept", &[a, b, c]) => sock_accept(proc, a.a()?, b.a()?, c.a()?) as _,
        ("sock_recv", &[a, b, c, d, e]) => {
            sock_recv(proc, a.a()?, b.a()?, c.a()?, d.a()?, e.a()?) as _
        }
        ("sock_send", &[a, b, c, d, e]) => {
            sock_send(proc, a.a()?, b.a()?, c.a()?, d.a()?, e.a()?) as _
        }
        ("sock_shutdown", &[a, b]) => sock_shutdown(proc, a.a()?, b.a()?) as _,
        ("poll_oneoff", &[a, b, c, d]) => poll_oneoff(proc, a.a()?, b.a()?, c.a()?, d.a()?) as _,
        ("thread_spawn", &[a]) => thread_spawn(proc, a.a()?),
        _ => return None,
    }))
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Immutable, IntoBytes)]
struct FdStatT {
    fs_filetype: FileType,
    _pad1: [u8; 1],
    fs_flags: FdFlags,
    _pad2: [u8; 4],
    fs_rights_base: Rights,
    fs_rights_inheriting: Rights,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Immutable, IntoBytes)]
struct FileStatT {
    dev: u64,
    inode: u64,
    filetype: FileType,
    pad1: [u8; 7],
    nlink: u64,
    size: FileSize,
    atim: Timestamp,
    mtim: Timestamp,
    ctim: Timestamp,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, IntoBytes, FromBytes)]
struct IoVecT {
    buf: Addr,
    buf_len: Size,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Immutable, IntoBytes)]
struct PreStatT {
    tag: u8,
    pad: [u8; 3],
    name_len: Size,
}

fn write_to_mem<T: IntoBytes + Immutable + ?Sized>(
    proc: &Process,
    addr: Addr,
    value: &T,
) -> Result<(), Errno> {
    let buffer = proc.memory.buffer();
    let array = Uint8Array::new(&buffer);
    let bytes = value.as_bytes();
    if bytes.len() as u64 + addr as u64 > array.length() as u64 {
        return Err(Errno::Fault);
    }
    let subarray = array.subarray(addr, addr + bytes.len() as u32);
    subarray.copy_from(bytes);
    Ok(())
}

fn read_from_mem<T: FromBytes + IntoBytes + ?Sized>(
    proc: &Process,
    addr: Addr,
    value: &mut T,
) -> Result<(), Errno> {
    let buffer = proc.memory.buffer();
    let array = Uint8Array::new(&buffer);
    let bytes = value.as_mut_bytes();
    if addr as u64 + bytes.len() as u64 > array.length() as u64 {
        return Err(Errno::Fault);
    }
    let subarray = array.subarray(addr, addr + bytes.len() as u32);
    subarray.copy_to(bytes);
    Ok(())
}

fn args_get(proc: &Process, argv: Addr, argv_buf: Addr) -> Errno {
    let mut argv_vec = vec![0; proc.args.len()];
    let mut offset = 0;
    for (i, arg) in proc.args.iter().enumerate() {
        if let Err(e) = write_to_mem(proc, argv_buf + offset, &arg[..]) {
            return e;
        }
        argv_vec[i] = argv_buf + offset;
        offset += arg.len() as Size;
    }
    if let Err(e) = write_to_mem(proc, argv, &argv_vec[..]) {
        return e;
    }
    Errno::Success
}

fn args_sizes_get(proc: &Process, argc: Addr, totarg: Addr) -> Errno {
    if let Err(e) = write_to_mem(proc, argc, &(proc.args.len() as Size)) {
        return e;
    }
    if let Err(e) = write_to_mem(
        proc,
        totarg,
        &(proc.args.iter().map(|s| s.len() as Size).sum::<Size>()),
    ) {
        return e;
    }
    Errno::Success
}

fn environ_get(proc: &Process, env: Addr, env_buf: Addr) -> Errno {
    let mut env_vec = vec![0; proc.env.len()];
    let mut offset = 0;
    for (i, e) in proc.env.iter().enumerate() {
        if let Err(e) = write_to_mem(proc, env_buf + offset, &e[..]) {
            return e;
        }
        env_vec[i] = env_buf + offset;
        offset += e.len() as Size;
    }
    if let Err(e) = write_to_mem(proc, env, &env_vec[..]) {
        return e;
    }
    Errno::Success
}

fn environ_sizes_get(proc: &Process, envc: Addr, totenv: Addr) -> Errno {
    if let Err(e) = write_to_mem(proc, envc, &(proc.env.len() as Size)) {
        return e;
    }
    if let Err(e) = write_to_mem(
        proc,
        totenv,
        &(proc.env.iter().map(|s| s.len() as Size).sum::<Size>()),
    ) {
        return e;
    }
    Errno::Success
}

fn clock_res_get(proc: &Process, clock_id: ClockId, out: Addr) -> Errno {
    let prec: Timestamp = match clock_id {
        ClockId::Monotonic => 1,
        ClockId::Realtime => 1,
        ClockId::ProcessCpu => return Errno::Inval,
        ClockId::ThreadCpu => return Errno::Inval,
    };
    if let Err(e) = write_to_mem(proc, out, &prec) {
        return e;
    }
    Errno::Success
}

fn clock_time_get(proc: &Process, clock_id: ClockId, _precision: Timestamp, time: Addr) -> Errno {
    let val = match clock_id {
        ClockId::Monotonic => web_time::UNIX_EPOCH.elapsed().unwrap().as_nanos() as Timestamp,
        ClockId::Realtime => proc.start_instant.elapsed().as_nanos() as Timestamp,
        ClockId::ProcessCpu => unimplemented!(),
        ClockId::ThreadCpu => unimplemented!(),
    };
    if let Err(e) = write_to_mem(proc, time, &val) {
        return e;
    }
    Errno::Success
}

fn fd_advise(
    _proc: &Process,
    _fd: Fd,
    _offset: FileSize,
    _len: FileSize,
    _advice: Advice,
) -> Errno {
    // Ignored.
    Errno::Success
}

fn fd_allocate(_proc: &Process, _fd: Fd, _offset: FileSize, _len: FileSize) -> Errno {
    Errno::Perm
}

fn fd_close(proc: &Process, fd: Fd) -> Errno {
    let mut proc_inner = proc.inner.borrow_mut();
    let fd_entry = proc_inner.fds.get_mut(fd as usize).map(Option::take);
    if fd_entry.flatten().is_none() {
        return Errno::Badf;
    }
    Errno::Success
}

fn fd_datasync(_proc: &Process, _fd: Fd) -> Errno {
    Errno::Success
}

fn fd_fdstat_get(proc: &Process, fd: Fd, buf: Addr) -> Errno {
    let proc_inner = proc.inner.borrow();
    let Some(Some(file_info)) = proc_inner.fds.get(fd as usize) else {
        return Errno::Badf;
    };
    let mut fdstat = FdStatT {
        fs_filetype: FileType::Unknown,
        _pad1: [0; 1],
        fs_flags: FdFlags::empty(),
        _pad2: [0; 4],
        fs_rights_base: Rights::empty(),
        fs_rights_inheriting: Rights::empty(),
    };
    match file_info {
        FdEntry::WriteFn(_) => {
            fdstat.fs_filetype = FileType::CharacterDevice;
            fdstat.fs_flags = FdFlags::APPEND;
            fdstat.fs_rights_base = Rights::FD_WRITE;
            fdstat.fs_rights_inheriting = Rights::FD_WRITE;
        }
        FdEntry::Data { .. } => {
            fdstat.fs_filetype = FileType::RegularFile;
            fdstat.fs_rights_base = Rights::FD_READ | Rights::FD_WRITE | Rights::FD_SEEK;
            fdstat.fs_rights_inheriting = Rights::FD_READ | Rights::FD_WRITE | Rights::FD_SEEK;
        }
        FdEntry::Dir(_) => {
            fdstat.fs_filetype = FileType::Directory;
            fdstat.fs_rights_base = Rights::PATH_OPEN
                | Rights::PATH_FILESTAT_GET
                | Rights::FD_FILESTAT_GET
                | Rights::PATH_READDIR;
            fdstat.fs_rights_inheriting = Rights::FD_READ
                | Rights::PATH_OPEN
                | Rights::PATH_FILESTAT_GET
                | Rights::FD_FILESTAT_GET
                | Rights::FD_SEEK
                | Rights::PATH_READDIR;
        }
        FdEntry::File(_, _, _) => {
            fdstat.fs_filetype = FileType::RegularFile;
            fdstat.fs_rights_base = Rights::FD_READ | Rights::FD_SEEK | Rights::FD_FILESTAT_GET;
            fdstat.fs_rights_inheriting =
                Rights::FD_READ | Rights::FD_WRITE | Rights::FD_FILESTAT_GET;
        }
        FdEntry::Pipe(_) => {
            fdstat.fs_filetype = FileType::CharacterDevice;
            fdstat.fs_flags = FdFlags::APPEND;
            fdstat.fs_rights_base = Rights::FD_READ | Rights::FD_WRITE;
            fdstat.fs_rights_inheriting = Rights::FD_READ | Rights::FD_WRITE;
        }
    };
    if let Err(e) = write_to_mem(proc, buf, &fdstat) {
        return e;
    }
    Errno::Success
}

fn fd_fdstat_set_flags(_proc: &Process, _fd: Fd, _flags: FdFlags) -> Errno {
    // Noop.
    Errno::Success
}

fn fd_fdstat_set_rights(_proc: &Process, _fd: Fd, _base: Rights, _inheriting: Rights) -> Errno {
    // TODO ?
    Errno::Success
}

fn fd_filestat_get(proc: &Process, fd: Fd, out: Addr) -> Errno {
    let proc_inner = proc.inner.borrow();
    let Some(Some(file_info)) = proc_inner.fds.get(fd as usize) else {
        return Errno::Badf;
    };
    let mut fstat = FileStatT {
        dev: 0,
        inode: 0,
        filetype: FileType::Unknown,
        pad1: [0; 7],
        nlink: 1,
        size: 0,
        atim: 0,
        mtim: 0,
        ctim: 0,
    };
    match file_info {
        FdEntry::WriteFn(_) => {
            fstat.dev = 1;
            fstat.filetype = FileType::CharacterDevice;
        }
        FdEntry::Data { data, .. } => {
            fstat.dev = 1;
            fstat.filetype = FileType::RegularFile;
            fstat.size = data.len() as FileSize;
        }
        FdEntry::Dir(inode) => {
            fstat.filetype = FileType::Directory;
            fstat.inode = *inode;
        }
        FdEntry::File(inode, _, _) => {
            fstat.filetype = FileType::RegularFile;
            fstat.inode = *inode;
            fstat.size = proc_inner.fs.entries[*inode as usize]
                .as_file()
                .unwrap()
                .len() as FileSize;
        }
        FdEntry::Pipe(_) => {
            fstat.dev = 1;
            fstat.filetype = FileType::CharacterDevice;
        }
    }
    if let Err(e) = write_to_mem(proc, out, &fstat) {
        return e;
    }
    Errno::Success
}

fn fd_filestat_set_size(_proc: &Process, _fd: Fd, _size: FileSize) -> Errno {
    Errno::Perm
}

fn fd_filestat_set_times(
    _proc: &Process,
    _fd: Fd,
    _atim: Timestamp,
    _mtim: Timestamp,
    _fst_flags: FstFlags,
) -> Errno {
    Errno::Perm
}

fn fd_pread(
    proc: &Process,
    fd: Fd,
    buf: Addr,
    buf_len: Size,
    offset: FileSize,
    result: Addr,
) -> Errno {
    let proc_inner = proc.inner.borrow();
    let Some(Some(file_entry)) = proc_inner.fds.get(fd as usize) else {
        return Errno::Badf;
    };
    let mut iovs = vec![IoVecT { buf: 0, buf_len: 0 }; buf_len as usize];
    if let Err(e) = read_from_mem(proc, buf, &mut iovs[..]) {
        return e;
    }
    let mut in_data = vec![0u8; iovs.iter().map(|iov| iov.buf_len).sum::<Size>() as usize];
    let read = match file_entry {
        FdEntry::Data { data, .. } => {
            let data = &data[offset as usize..];
            let read_len = data.len().min(in_data.len());
            in_data[..read_len].copy_from_slice(&data[..read_len]);
            read_len
        }
        FdEntry::File(inode, _, _) => {
            let data = proc_inner.fs.entries[*inode as usize].as_file().unwrap();
            let data = &data[offset as usize..];
            let read_len = data.len().min(in_data.len());
            in_data[..read_len].copy_from_slice(&data[..read_len]);
            read_len
        }
        FdEntry::WriteFn(_) => return Errno::Badf,
        FdEntry::Dir(_) => return Errno::Badf,
        FdEntry::Pipe(_) => return Errno::Badf,
    };
    let mut pos = 0;
    for IoVecT { buf, buf_len } in iovs {
        if let Err(e) = write_to_mem(proc, buf, &in_data[pos..pos + buf_len as usize]) {
            return e;
        }
        pos += buf_len as usize;
    }
    if let Err(e) = write_to_mem(proc, result, &read) {
        return e;
    }
    Errno::Success
}

fn fd_prestat_get(proc: &Process, fd: Fd, out: Addr) -> Errno {
    let proc_inner = proc.inner.borrow();
    let Some(Some(file_entry)) = proc_inner.fds.get(fd as usize) else {
        return Errno::Badf;
    };
    let FdEntry::Dir(inode) = *file_entry else {
        return Errno::Badf;
    };
    let name = proc_inner.fs.get_name(inode);
    let prestat = PreStatT {
        tag: 0,
        pad: [0; 3],
        name_len: name.len() as Size,
    };
    if let Err(e) = write_to_mem(proc, out, &prestat) {
        return e;
    }
    Errno::Success
}

fn fd_prestat_dir_name(proc: &Process, fd: Fd, path: Addr, _path_len: Size) -> Errno {
    let proc_inner = proc.inner.borrow();
    let Some(Some(file_entry)) = proc_inner.fds.get(fd as usize) else {
        return Errno::Badf;
    };
    let FdEntry::Dir(inode) = *file_entry else {
        return Errno::Badf;
    };
    let name = proc_inner.fs.get_name(inode);
    if let Err(e) = write_to_mem(proc, path, name.as_slice()) {
        return e;
    }
    Errno::Success
}

fn fd_pwrite(
    proc: &Process,
    fd: Fd,
    iovs_addr: Addr,
    iovs_len: Size,
    offset: FileSize,
    out: Addr,
) -> Errno {
    let mut proc_inner = proc.inner.borrow_mut();
    let ProcessInner { fds, fs, .. } = &mut *proc_inner;
    let Some(Some(fd_entry)) = fds.get_mut(fd as usize) else {
        return Errno::Badf;
    };
    let mut iovs = vec![IoVecT { buf: 0, buf_len: 0 }; iovs_len as usize];
    if let Err(e) = read_from_mem(proc, iovs_addr, &mut iovs[..]) {
        return e;
    }
    let in_data_len = iovs.iter().map(|iov| iov.buf_len).sum::<Size>();
    let mut in_data = vec![0u8; in_data_len as usize];
    let mut pos = 0;
    for IoVecT { buf, buf_len } in iovs {
        if let Err(e) = read_from_mem(proc, buf, &mut in_data[pos..pos + buf_len as usize]) {
            return e;
        }
        pos += buf_len as usize;
    }
    let written = match fd_entry {
        FdEntry::Data { data, .. } => {
            let end = offset as usize + in_data.len();
            if end > data.len() {
                data.resize(end, 0);
            }
            data[offset as usize..end].copy_from_slice(&in_data);
            in_data.len()
        }
        FdEntry::File(inode, _, _) => {
            let file_entry = fs.entries[*inode as usize].as_file_mut().unwrap();
            let data = Rc::make_mut(file_entry);
            let end = offset as usize + in_data.len();
            if end > data.len() {
                data.resize(end, 0);
            }
            data[offset as usize..end].copy_from_slice(&in_data);
            in_data.len()
        }
        FdEntry::WriteFn(_) => return Errno::Badf,
        FdEntry::Pipe(_) => return Errno::Badf,
        FdEntry::Dir(_) => return Errno::Badf,
    };
    if let Err(e) = write_to_mem(proc, out, &written) {
        return e;
    }
    Errno::Success
}

async fn fd_read(proc: &Process, fd: Fd, buf: Addr, buf_len: Size, result: Addr) -> Errno {
    let mut iovs = vec![IoVecT { buf: 0, buf_len: 0 }; buf_len as usize];
    if let Err(e) = read_from_mem(proc, buf, &mut iovs[..]) {
        return e;
    }
    let mut in_data = vec![0u8; iovs.iter().map(|iov| iov.buf_len).sum::<Size>() as usize];
    let mut pipe = None;
    let mut read = {
        let mut proc_inner = proc.inner.borrow_mut();
        let ProcessInner { fds, fs, .. } = &mut *proc_inner;
        let Some(Some(file_entry)) = fds.get_mut(fd as usize) else {
            return Errno::Badf;
        };
        match file_entry {
            FdEntry::Data { data, offset } => {
                let data = &data[*offset..];
                let read_len = data.len().min(in_data.len());
                in_data[..read_len].copy_from_slice(&data[..read_len]);
                *offset += read_len;
                read_len
            }
            FdEntry::File(inode, offset, _) => {
                let data = fs.entries[*inode as usize].as_file().unwrap();
                let data = &data[*offset..];
                let read_len = data.len().min(in_data.len());
                in_data[..read_len].copy_from_slice(&data[..read_len]);
                *offset += read_len;
                read_len
            }
            FdEntry::Pipe(p) => {
                pipe = Some(p.clone());
                0
            }
            FdEntry::WriteFn(_) => return Errno::Badf,
            FdEntry::Dir(_) => return Errno::Badf,
        }
    };
    if let Some(pipe) = pipe {
        read = pipe.read(&mut in_data).await;
    }
    let mut pos = 0;
    for IoVecT { buf, buf_len } in iovs {
        if let Err(e) = write_to_mem(proc, buf, &in_data[pos..pos + buf_len as usize]) {
            return e;
        }
        pos += buf_len as usize;
    }
    if let Err(e) = write_to_mem(proc, result, &read) {
        return e;
    }
    Errno::Success
}

fn fd_readdir(
    proc: &Process,
    fd: Fd,
    buf_addr: Addr,
    buf_len: Size,
    cookie: u64,
    out: Addr,
) -> Errno {
    let proc_inner = proc.inner.borrow();
    let Some(Some(fd_entry)) = proc_inner.fds.get(fd as usize) else {
        return Errno::Badf;
    };
    let FdEntry::Dir(dir_inode) = *fd_entry else {
        return Errno::Badf;
    };
    let mut buf = Vec::new();
    let entries = proc_inner.fs.entries[dir_inode as usize].as_dir().unwrap();
    for (idx, (name, inode)) in entries.iter().enumerate().skip(cookie as usize) {
        if buf.len() >= buf_len as usize {
            break;
        }
        buf.extend_from_slice(&(idx as u64 + 1).to_le_bytes());
        buf.extend_from_slice(&inode.to_le_bytes());
        buf.extend_from_slice(&(name.len() as u32).to_le_bytes());
        let file_type = match proc_inner.fs.entries[*inode as usize] {
            FsEntry::File(_) => FileType::RegularFile,
            FsEntry::Dir(_) => FileType::Directory,
            FsEntry::Pipe(_) => FileType::CharacterDevice,
        };
        buf.extend_from_slice(&(file_type as u32).to_le_bytes());
        buf.extend_from_slice(name);
    }
    let len = buf.len().min(buf_len as usize);
    if let Err(e) = write_to_mem(proc, buf_addr, &buf[..len]) {
        return e;
    }
    if let Err(e) = write_to_mem(proc, out, &(len as Size)) {
        return e;
    }
    Errno::Success
}

fn fd_renumber(_proc: &Process, _fd: Fd, _to_fd: Fd) -> Errno {
    // Maybe?
    Errno::Perm
}

fn fd_seek(proc: &Process, fd: Fd, offset: FileDelta, whence: Whence, out: Addr) -> Errno {
    let mut proc_inner = proc.inner.borrow_mut();
    let ProcessInner { fds, fs, .. } = &mut *proc_inner;
    let Some(Some(file_info)) = fds.get_mut(fd as usize) else {
        return Errno::Badf;
    };
    let mut base_off: FileSize = match (whence, &*file_info) {
        (Whence::Set, _) => 0,
        (Whence::Cur, _) => 0,
        (Whence::End, FdEntry::Data { data, .. }) => data.len() as FileSize,
        (Whence::End, FdEntry::File(inode, _, _)) => {
            fs.entries[*inode as usize].as_file().unwrap().len() as FileSize
        }
        _ => return Errno::Inval,
    };
    let foff = match file_info {
        FdEntry::Data { offset, .. } => offset,
        FdEntry::File(_, offset, _) => offset,
        FdEntry::WriteFn(_) => return Errno::Badf,
        FdEntry::Dir(_) => return Errno::Badf,
        FdEntry::Pipe(_) => return Errno::Badf,
    };
    if whence == Whence::Cur {
        base_off = *foff as FileSize;
    }
    *foff = (base_off as usize).saturating_add_signed(offset as isize);
    if let Err(e) = write_to_mem(proc, out, &(*foff as FileSize)) {
        return e;
    }
    Errno::Success
}

fn fd_sync(_proc: &Process, _fd: Fd) -> Errno {
    Errno::Success
}

fn fd_tell(proc: &Process, fd: Fd, out: Addr) -> Errno {
    fd_seek(proc, fd, 0, Whence::Cur, out)
}

fn fd_write(proc: &Process, fd: Fd, iovs_addr: Addr, iovs_len: Size, result: Addr) -> Errno {
    let mut proc_inner = proc.inner.borrow_mut();
    let ProcessInner { fds, fs, .. } = &mut *proc_inner;
    let Some(Some(fd_entry)) = fds.get_mut(fd as usize) else {
        return Errno::Badf;
    };
    let mut iovs = vec![IoVecT { buf: 0, buf_len: 0 }; iovs_len as usize];
    if let Err(e) = read_from_mem(proc, iovs_addr, &mut iovs[..]) {
        return e;
    }
    let in_data_len = iovs.iter().map(|iov| iov.buf_len).sum::<Size>();
    let mut in_data = vec![0u8; in_data_len as usize];
    let mut pos = 0;
    for IoVecT { buf, buf_len } in iovs {
        if let Err(e) = read_from_mem(proc, buf, &mut in_data[pos..pos + buf_len as usize]) {
            return e;
        }
        pos += buf_len as usize;
    }
    let written = match fd_entry {
        FdEntry::WriteFn(f) => f(&in_data),
        FdEntry::Data { data, offset } => {
            let end = *offset + in_data.len();
            if end > data.len() {
                data.resize(end, 0);
            }
            data[*offset..end].copy_from_slice(&in_data);
            *offset = end;
            in_data.len()
        }
        FdEntry::Pipe(pipe) => {
            pipe.write(&in_data);
            in_data.len()
        }
        FdEntry::File(inode, offset, append) => {
            let file_entry = fs.entries[*inode as usize].as_file_mut().unwrap();
            let data = Rc::make_mut(file_entry);
            if *append {
                *offset = data.len();
            }
            let end = *offset + in_data.len();
            if end > data.len() {
                data.resize(end, 0);
            }
            data[*offset..end].copy_from_slice(&in_data);
            *offset = end;
            in_data.len()
        }
        FdEntry::Dir(_) => return Errno::Badf,
    };
    if let Err(e) = write_to_mem(proc, result, &written) {
        return e;
    }
    Errno::Success
}

fn path_create_directory(_proc: &Process, _fd: Fd, _path: Addr, _path_len: Size) -> Errno {
    Errno::Perm
}

fn path_filestat_get(
    proc: &Process,
    fd: Fd,
    _flags: LookupFlags,
    path_addr: Addr,
    path_len: Size,
    filestat: Addr,
) -> Errno {
    let proc_inner = proc.inner.borrow();
    let Some(Some(file_entry)) = proc_inner.fds.get(fd as usize) else {
        return Errno::Badf;
    };
    let FdEntry::Dir(base_inode) = *file_entry else {
        return Errno::Badf;
    };
    let mut path = vec![0; path_len as usize];
    if let Err(e) = read_from_mem(proc, path_addr, &mut path[..]) {
        return e;
    }
    let inode = match proc_inner.fs.get(base_inode, &path) {
        Ok(inode) => inode,
        Err(FsError::DoesNotExist) => return Errno::NoEnt,
        Err(FsError::NotDir) => return Errno::NotDir,
        Err(FsError::IsDir) => return Errno::IsDir,
        Err(FsError::Exist) => return Errno::Exist,
    };
    let mut fstat = FileStatT {
        dev: 0,
        inode,
        filetype: FileType::Unknown,
        pad1: [0; 7],
        nlink: 1,
        size: 0,
        atim: 0,
        mtim: 0,
        ctim: 0,
    };
    match proc_inner.fs.entries[inode as usize] {
        FsEntry::Dir(_) => {
            fstat.filetype = FileType::Directory;
        }
        FsEntry::File(ref file) => {
            fstat.filetype = FileType::RegularFile;
            fstat.size = file.len() as FileSize;
        }
        FsEntry::Pipe(_) => {
            fstat.filetype = FileType::CharacterDevice;
        }
    }
    if let Err(e) = write_to_mem(proc, filestat, &fstat) {
        return e;
    }
    Errno::Success
}

#[allow(clippy::too_many_arguments)]
fn path_filestat_set_times(
    _proc: &Process,
    _fd: Fd,
    _flags: LookupFlags,
    _path: Addr,
    _path_len: Size,
    _atim: Timestamp,
    _mtim: Timestamp,
    _fst_flags: FstFlags,
) -> Errno {
    Errno::Perm
}

fn path_link(
    _proc: &Process,
    _fd: Fd,
    _old_path: Addr,
    _old_path_len: Size,
    _new_fd: Fd,
    _new_path: Addr,
    _new_path_len: Size,
) -> Errno {
    Errno::Perm
}

#[allow(clippy::too_many_arguments)]
fn path_open(
    proc: &Process,
    dirfd: Fd,
    _dir_flags: LookupFlags,
    path_ptr: Addr,
    path_len: Size,
    oflags: OFlags,
    _rights_base: Rights,
    _rights_inheriting: Rights,
    fd_flags: FdFlags,
    out: Addr,
) -> Errno {
    let mut proc_inner = proc.inner.borrow_mut();
    let ProcessInner { fds, fs, .. } = &mut *proc_inner;
    let Some(Some(file_entry)) = fds.get_mut(dirfd as usize) else {
        return Errno::Badf;
    };
    let FdEntry::Dir(base_inode) = *file_entry else {
        return Errno::Badf;
    };
    let mut path = vec![0; path_len as usize];
    if let Err(e) = read_from_mem(proc, path_ptr, &mut path[..]) {
        return e;
    }
    let inode = match fs.open(
        base_inode,
        &path,
        oflags.contains(OFlags::CREAT),
        oflags.contains(OFlags::EXCL),
    ) {
        Ok(inode) => inode,
        Err(FsError::DoesNotExist) => return Errno::NoEnt,
        Err(FsError::NotDir) => return Errno::NotDir,
        Err(FsError::IsDir) => return Errno::IsDir,
        Err(FsError::Exist) => return Errno::Exist,
    };
    if oflags.contains(OFlags::DIRECTORY) && !matches!(fs.entries[inode as usize], FsEntry::Dir(_))
    {
        return Errno::NotDir;
    };
    let file_entry = match &mut fs.entries[inode as usize] {
        FsEntry::Dir(_) => FdEntry::Dir(inode),
        FsEntry::File(data) => {
            if oflags.contains(OFlags::TRUNC) {
                *data = Rc::new(Vec::new());
            }
            FdEntry::File(inode, 0, fd_flags.contains(FdFlags::APPEND))
        }
        FsEntry::Pipe(p) => FdEntry::Pipe(p.clone()),
    };
    let fd = proc_inner.add_fd(file_entry);
    if let Err(e) = write_to_mem(proc, out, &fd) {
        return e;
    }
    Errno::Success
}

fn path_readlink(
    _proc: &Process,
    _fd: Fd,
    _path: Addr,
    _path_len: Size,
    _buf: Addr,
    _buf_len: Size,
    _out: Addr,
) -> Errno {
    Errno::Perm
}

fn path_remove_directory(_proc: &Process, _fd: Fd, _path: Addr, _path_len: Size) -> Errno {
    Errno::Perm
}

fn path_rename(
    _proc: &Process,
    _fd: Fd,
    _old_path: Addr,
    _old_path_len: Size,
    _new_fd: Fd,
    _new_path: Addr,
    _new_path_len: Size,
) -> Errno {
    Errno::Perm
}

fn path_symlink(
    _proc: &Process,
    _source: Addr,
    _source_len: Size,
    _fd: Fd,
    _path: Addr,
    _path_len: Size,
) -> Errno {
    Errno::Perm
}

fn path_unlink_file(proc: &Process, fd: Fd, path_addr: Addr, path_len: Size) -> Errno {
    // TODO: we are leaking files here
    let mut proc_inner = proc.inner.borrow_mut();
    let Some(Some(file_entry)) = proc_inner.fds.get_mut(fd as usize) else {
        return Errno::Badf;
    };
    let FdEntry::Dir(base_inode) = *file_entry else {
        return Errno::Badf;
    };
    let mut path = vec![0; path_len as usize];
    if let Err(e) = read_from_mem(proc, path_addr, &mut path[..]) {
        return e;
    }
    let inode = match proc_inner.fs.get(base_inode, &path) {
        Ok(inode) => inode,
        Err(FsError::DoesNotExist) => return Errno::NoEnt,
        Err(FsError::NotDir) => return Errno::NotDir,
        Err(FsError::IsDir) => return Errno::IsDir,
        Err(FsError::Exist) => return Errno::Exist,
    };
    if inode == proc_inner.fs.root() {
        return Errno::Perm;
    }
    let parent = proc_inner.fs.parent_pointers[inode as usize];
    proc_inner.fs.entries[parent as usize]
        .as_dir_mut()
        .unwrap()
        .retain(|_, c| *c != inode);
    Errno::Success
}

fn proc_exit(proc: &Process, code: ExitCode) {
    proc.kill(StatusCode::Exited(code));
}

fn proc_raise(_proc: &Process, _code: Signal) -> Errno {
    todo!()
}

fn random_get(proc: &Process, buf_addr: Addr, buf_len: Size) -> Errno {
    let mut buf = vec![0u8; buf_len as usize];
    for byte in buf.iter_mut() {
        *byte = (js_sys::Math::random() * 256.) as u8;
    }
    if let Err(e) = write_to_mem(proc, buf_addr, &buf[..]) {
        return e;
    }
    Errno::Success
}

fn sched_yield(_proc: &Process) -> Errno {
    Errno::Success
}

fn sock_accept(_proc: &Process, _fd: Fd, _addr: Addr, _addr_len: Addr) -> Errno {
    Errno::Perm
}

fn sock_recv(
    _proc: &Process,
    _fd: Fd,
    _buf: Addr,
    _buf_len: Size,
    _flags: Addr,
    _result: Addr,
) -> Errno {
    Errno::Perm
}

fn sock_send(
    _proc: &Process,
    _fd: Fd,
    _buf: Addr,
    _buf_len: Size,
    _flags: Addr,
    _result: Addr,
) -> Errno {
    Errno::Perm
}

fn sock_shutdown(proc: &Process, fd: Fd, _how: u8) -> Errno {
    let proc_inner = proc.inner.borrow();
    let Some(Some(_sock)) = proc_inner.fds.get(fd as usize) else {
        return Errno::Badf;
    };
    // Sockets do not exists
    Errno::NotSock
}

fn poll_oneoff(
    _proc: &Process,
    _subs_addr: Addr,
    _events_addr: Addr,
    _num_subs: Size,
    _out: Addr,
) -> Errno {
    Errno::Perm
}

fn thread_spawn(proc: &Rc<Process>, attr: i32) -> i32 {
    proc.spawn_thread(Some(attr)) as i32
}
