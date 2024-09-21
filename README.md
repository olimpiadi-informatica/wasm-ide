# wasm-ide
A simple wasm-based IDE using compilers from
https://github.com/olimpiadi-informatica/wasm-compilers.

The IDE needs COEP and COOP to work properly. You can set this up i.e. with nginx as follows:

```
add_header 'Cross-Origin-Embedder-Policy' 'require-corp';
add_header 'Cross-Origin-Opener-Policy' 'same-origin';
```

## Dependencies
Install `rustup` and `cargo`, and ensure `~/.cargo/bin` is in your `PATH`. Then:

```
rustup target add wasm32-unknown-unknown
cargo install wasm-pack wasm-opt
cargo install --locked trunk
```

## Installation
Build the artefacts from https://github.com/olimpiadi-informatica/wasm-compilers/ and
place them in a folder named `compilers` in the root of this repository.

Run `trunk build --release`.

## Development
```
trunk serve $(find worker frontend common -type f | xargs -n 1 echo -w) \
    -w Cargo.toml -w style/main.scss -w index.html -w start_worker.js -w start_worker_thread.js \
    -w codemirror_interface.ts -w Trunk.toml --release
```

Note: you still need to add COEP and COOP headers.

## Initial code and stdin
If files `frontend/code.txt` and `frontend/stdin.txt` exist at compilation time, they will be
used as the code/stdin shown to first-time users.
