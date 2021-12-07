build:
# more about flags: https://github.com/near-examples/simulation-testing#gotchas
# env setting instruments cargo to optimize the the build for size (link-args=-s)
	@env 'RUSTFLAGS=-C link-arg=-s' cargo build --lib --target wasm32-unknown-unknown --release
	cp target/wasm32-unknown-unknown/release/*.wasm ./res/