.PHONY: default

default:
	cp -r target/doc/* .
	git rm *.module.wasm; cp -r wasm/demo/dist/* . ; git add *.module.wasm
