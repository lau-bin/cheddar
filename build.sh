#!/bin/bash
set -e

RUSTFLAGS='-C link-arg=-s' cargo +stable build --all --target wasm32-unknown-unknown --release

cp target/wasm32-unknown-unknown/release/cookie-factory-pool.wasm ./res
cp target/wasm32-unknown-unknown/release/test-token-nep145.wasm ./res
