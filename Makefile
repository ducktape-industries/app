.PHONY: views run dev

views:
	ops/stage-views.sh

run: views
	cargo run -p ducktape-app

dev: run
