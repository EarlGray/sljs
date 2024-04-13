.PHONY: site

site:
	mkdir -p site
	cargo doc --no-deps && cp -r target/doc/* site/
	cd wasm && wasm-pack build
	cd wasm/demo && npm install && npx webpack
	cp -r wasm/demo/dist/* site/
