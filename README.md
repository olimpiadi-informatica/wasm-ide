# wasm-ide

`wasm-ide` is a browser-based IDE for competitive programming, built on top of
WebAssembly. It compiles and runs code inside a Web Worker, stores workspaces in
the browser filesystem, and can optionally integrate with remote evaluation and
contest systems.

## How to build

First install [`rustup`](https://rustup.rs/) and make sure `~/.cargo/bin` is in
your `PATH`.

Then install the required Rust tooling:

```bash
rustup target add wasm32-unknown-unknown
cargo install --locked trunk
```

You also need these tools available in `PATH`:

- `npm`
- `jq`
- `brotli`

Runtime configuration is loaded from `config.json`. If that file is missing, the
build falls back to `config.example.json`.

The main configuration keys are:

- `default_ws`: files created for a new local workspace
- `remote_eval`: optional remote evaluation endpoint
- `terry`: optional contest-system endpoint

Compiler artifacts must be downloaded from
[`olimpiadi-informatica/wasm-compilers`](https://github.com/olimpiadi-informatica/wasm-compilers)
and placed in `./compilers`. The build expects the `.tar.br` files there.

Then build the project with:

```bash
trunk build --release
```

## How to serve

The app requires COEP and COOP headers.

A minimal `nginx` configuration for serving the generated `dist/` directory is:

```nginx
server {
    listen 80;
    server_name _;

    root /path/to/wasm-ide/dist;
    index index.html;

    add_header Cross-Origin-Embedder-Policy require-corp;
    add_header Cross-Origin-Opener-Policy same-origin;

    location / {
    }

    location /compilers/ {
        brotli_static on;
    }
}
```

`brotli_static on;` is useful if you want `nginx` to serve precompressed
compiler archives directly.
