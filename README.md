# wasm-ide

`wasm-ide` is a browser-based IDE for competitive programming. It compiles and
runs code in a Web Worker, stores workspaces in the browser filesystem, and can
optionally connect to remote evaluation and contest systems.

## Using a precompiled release

Download `wasm-ide.tar.gz` or `wasm-ide.zip` from the
[latest release](https://github.com/olimpiadi-informatica/wasm-ide/releases/latest),
extract it into a directory served by your web server, and adjust `config.json`
as needed.

The application must be served over HTTPS; opening `index.html` directly is not
supported. It also requires cross-origin isolation, so every response must
include these headers:

```text
Cross-Origin-Embedder-Policy: require-corp
Cross-Origin-Opener-Policy: same-origin
```

Compiler archives in the release are Brotli-compressed. Requests for
`/compilers/<language>.tar` must be served from the corresponding `.tar.br`
file with Brotli content encoding. For example, an nginx server with the
[`ngx_brotli`](https://github.com/google/ngx_brotli) module can use:

```nginx
server {
    listen 80;
    server_name _;

    root /path/to/wasm-ide;
    index index.html;

    add_header Cross-Origin-Embedder-Policy require-corp always;
    add_header Cross-Origin-Opener-Policy same-origin always;

    location / {
        try_files $uri $uri/ =404;
    }

    location /compilers/ {
        brotli_static on;
    }
}
```

Use HTTPS for a public deployment. TLS configuration is omitted from this
minimal example.

## Configuration

The frontend loads `config.json` from the web root at startup. The release
archive includes a ready-to-use configuration; source builds use `config.json`
when present and otherwise fall back to [`config.example.json`](config.example.json).

The main fields are:

- `default_ws`: initial `code` and `stdin` files for a new local workspace. Each
  object maps a filename to either a UTF-8 string or an array of bytes.
- `remote_eval`: an optional endpoint for a remote evaluation backend. Set it
  to `null` to use only the in-browser backends.
- `contest`: an optional contest-system connection. Set it to `null` when no
  contest integration is needed.
- `compilers`: the generated map of compiler archive names to their
  uncompressed sizes. Preserve this field in a precompiled release. Source
  builds generate it from the archives in `compilers/`.

Contest configuration is internally tagged. For CMS:

```json
{
  "contest": {
    "type": "cms",
    "endpoint": "https://cms.example"
  }
}
```

For Terry, use `"type": "terry"` with the appropriate endpoint. Only one
contest system can be configured at a time.

## Evaluation backends

By default, wasm-ide compiles and runs C, C++, Python, Rust, and JavaScript
locally in the browser. Additional languages can be made available through a
remote evaluator by setting `remote_eval` in `config.json`.

The recommended remote evaluator is `eval-server` from
[`olimpiadi-informatica/task-maker-rust`](https://github.com/olimpiadi-informatica/task-maker-rust).
Remote evaluation does not support streaming input or language-server features.

## Compiling from source

Install [Rust](https://rustup.rs/) and ensure `~/.cargo/bin` is in `PATH`. The
repository's toolchain file selects stable Rust and the WebAssembly target; then
install Trunk:

```bash
rustup target add wasm32-unknown-unknown
cargo install --locked trunk
```

The build also requires `npm`, `jq`, and `brotli` in `PATH`.

Download the `.tar.br` compiler archives from
[`olimpiadi-informatica/wasm-compilers`](https://github.com/olimpiadi-informatica/wasm-compilers/releases)
and place them in `compilers/`. With the GitHub CLI installed, this can be done
with:

```bash
mkdir -p compilers
gh release download --repo olimpiadi-informatica/wasm-compilers \
  --dir compilers --pattern '*.tar.br'
```

Optionally create `config.json` to override the example configuration, then
build the production bundle:

```bash
trunk build --release
```

The complete static application is written to `dist/`. Serve that directory
with the headers and Brotli handling described above.

## Adding a contest integration

Contest integrations live in
[`frontend/src/contest_api`](frontend/src/contest_api). Start with the module
documentation in
[`frontend/src/contest_api/mod.rs`](frontend/src/contest_api/mod.rs), which
describes the extension process and the `ContestAPI` contract.

In short, a new integration needs a protocol-specific module, a
`ContestConfig` variant, an implementation of the `ContestAPI` trait, and a
constructor entry in `contest_api::init`. The existing CMS and Terry modules are
working examples.
