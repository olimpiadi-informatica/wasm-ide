#!/usr/bin/env bash
set -ueo pipefail

SELFDIR="$(realpath "$(dirname "$0")")"
TMPDIR=$(mktemp -d)

cleanup() {
    rm -rf $TMPDIR
}
trap cleanup EXIT

git clone 'https://github.com/WebAssembly/wasi-testsuite' --single-branch $TMPDIR/wasi-testsuite
cd $TMPDIR/wasi-testsuite/tests
git checkout 706f17a

pushd assemblyscript
npm install
for input in testsuite/*.ts; do
  output="testsuite/$(basename $input .ts).wasm"
  echo "Compiling $input"
  npm run asc -- --enable threads --maximumMemory=65536 --importMemory --sharedMemory "$input" -o "$output"
done
cd testsuite
tar cf "$SELFDIR/as.tar" .
popd

pushd c
CC=${CC:=clang}
for input in testsuite/*.c; do
  output="testsuite/$(basename $input .c).wasm"
  echo "Compiling $input"
  $CC --target=wasm32-wasip1-threads -Xclang -target-feature -Xclang +atomics -Xclang -target-feature -Xclang +bulk-memory -Xclang -target-feature -Xclang +mutable-globals -Wl,--shared-memory -Wl,--import-memory -Wl,--max-memory=4294967296 "$input" -o "$output"
done
cd testsuite
tar cf "$SELFDIR/c.tar" .
popd

pushd rust
RUSTFLAGS="-Clink-arg=--max-memory=4294967296" cargo build --target=wasm32-wasip1-threads
cp target/wasm32-wasip1-threads/debug/*.wasm testsuite/
cd testsuite
tar cf "$SELFDIR/rs.tar" .
popd
