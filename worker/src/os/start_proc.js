let channel;

function syscall(kind) {
    return function(...args) {
        let array = new Int32Array(channel);
        Atomics.store(array, 0, -1);
        postMessage({ kind, args });
        Atomics.wait(array, 0, -1);
        const val = Atomics.load(array, 0);
        return val;
    };
}

const wasip1_names = [
    "args_get", "args_sizes_get", "clock_res_get", "clock_time_get", "environ_get",
    "environ_sizes_get", "fd_advise", "fd_allocate", "fd_close", "fd_datasync", "fd_fdstat_get",
    "fd_fdstat_set_flags", "fd_fdstat_set_rights", "fd_filestat_get", "fd_filestat_set_size",
    "fd_filestat_set_times", "fd_pread", "fd_prestat_dir_name", "fd_prestat_get", "fd_pwrite",
    "fd_read", "fd_readdir", "fd_renumber", "fd_seek", "fd_sync", "fd_tell", "fd_write",
    "path_create_directory", "path_filestat_get", "path_filestat_set_times", "path_link",
    "path_open", "path_readlink", "path_remove_directory", "path_rename", "path_symlink",
    "path_unlink_file", "poll_oneoff", "proc_exit", "proc_raise", "random_get", "sched_yield",
    "sock_accept", "sock_recv", "sock_send", "sock_shutdown",
];

const wasip1 = Object.fromEntries(
    wasip1_names.map(name => [name, syscall(name)])
);

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
    try {
        if (msg.data.tid !== undefined) {
            wasm.exports.wasi_thread_start(msg.data.tid, msg.data.arg);
        } else {
            wasm.exports._start();
            postMessage({ kind: 'proc_exit', args: [0] });
        }
    } catch (e) {
        postMessage({ re: e.message });
    }

    self.close();
};
