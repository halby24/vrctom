#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"
cargo build -p vrctom-wasm --target wasm32-unknown-unknown --release --locked
wasm-bindgen --target web --out-dir web/src/wasm --out-name vrctom_wasm \
  target/wasm32-unknown-unknown/release/vrctom_wasm.wasm
