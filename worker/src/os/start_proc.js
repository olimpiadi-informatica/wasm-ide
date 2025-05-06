let channel;

function syscall(name) {
    return function(...args) {
        // TODO: Make sure bigints are passed correctly
        args = args.map(arg => Number(arg));
        //console.log(`syscall: ${name}(${args.join(", ")})`);
        let array = new Int32Array(channel);
        Atomics.store(array, 0, -1);
        postMessage({ type: name, args: (args.length === 0 ? undefined : args.length === 1 ? args[0] : args) });
        Atomics.wait(array, 0, -1);
        const val = Atomics.load(array, 0);
        return val;
    };
}

const wasip1 = {
    args_get: syscall("args_get"),
    args_sizes_get: syscall("args_sizes_get"),
    environ_get: syscall("environ_get"),
    environ_sizes_get: syscall("environ_sizes_get"),
    clock_res_get: syscall("clock_res_get"),
    clock_time_get: syscall("clock_time_get"),
    fd_advise: syscall("fd_advise"),
    fd_allocate: syscall("fd_allocate"),
    fd_close: syscall("fd_close"),
    fd_datasync: syscall("fd_datasync"),
    fd_fdstat_get: syscall("fd_fdstat_get"),
    fd_fdstat_set_flags: syscall("fd_fdstat_set_flags"),
    fd_fdstat_set_rights: syscall("fd_fdstat_set_rights"),
    fd_filestat_get: syscall("fd_filestat_get"),
    fd_filestat_set_size: syscall("fd_filestat_set_size"),
    fd_filestat_set_times: syscall("fd_filestat_set_times"),
    fd_pread: syscall("fd_pread"),
    fd_prestat_get: syscall("fd_prestat_get"),
    fd_prestat_dir_name: syscall("fd_prestat_dir_name"),
    fd_pwrite: syscall("fd_pwrite"),
    fd_read: syscall("fd_read"),
    fd_readdir: syscall("fd_readdir"),
    fd_renumber: syscall("fd_renumber"),
    fd_seek: syscall("fd_seek"),
    fd_sync: syscall("fd_sync"),
    fd_tell: syscall("fd_tell"),
    fd_write: syscall("fd_write"),
    path_create_directory: syscall("path_create_directory"),
    path_filestat_get: syscall("path_filestat_get"),
    path_filestat_set_times: syscall("path_filestat_set_times"),
    path_link: syscall("path_link"),
    path_open: syscall("path_open"),
    path_readlink: syscall("path_readlink"),
    path_remove_directory: syscall("path_remove_directory"),
    path_rename: syscall("path_rename"),
    path_symlink: syscall("path_symlink"),
    path_unlink_file: syscall("path_unlink_file"),
    proc_exit: syscall("proc_exit"),
    proc_raise: syscall("proc_raise"),
    random_get: syscall("random_get"),
    sched_yield: syscall("sched_yield"),
    sock_accept: syscall("sock_accept"),
    sock_recv: syscall("sock_recv"),
    sock_send: syscall("sock_send"),
    sock_shutdown: syscall("sock_shutdown"),
    poll_oneoff: syscall("poll_oneoff")
};

const wasi = {
    'thread-spawn': syscall("thread_spawn"),
};

self.onmessage = (msg) => {
    const imports = {
        wasi_snapshot_preview1: wasip1,
        wasi: wasi,
        env: {
            memory: msg.data.memory,
        }
    };
    channel = msg.data.channel;
    let wasm = new WebAssembly.Instance(msg.data.module, imports);
    if (msg.data.tid !== undefined) {
        wasm.exports.wasi_thread_start(msg.data.tid, msg.data.arg);
    } else {
        wasm.exports._start();
    }
    wasip1.proc_exit(0);
};
