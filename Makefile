.PHONY: install

CARGO ?= cargo
export CARGO
ifneq ($(origin CARGO_TARGET_DIR), undefined)
export CARGO_TARGET_DIR
endif
ifneq ($(origin CARGO_BUILD_JOBS), undefined)
export CARGO_BUILD_JOBS
endif

install:
	@./ops/install-app.sh
