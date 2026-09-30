SHELL := /bin/bash

PARLANDO_DIR := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))

RUST_SERVER_DIR := $(PARLANDO_DIR)/rust-server
RUST_SERVER_TESTS_DIR := $(PARLANDO_DIR)/rust-server-tests
CLIENT_SERVER_TESTS_DIR := $(PARLANDO_DIR)/client-server-tests
JS_CLIENT_DIR := $(PARLANDO_DIR)/js-client
PYTHON_AGENT_SDK_DIR := $(PARLANDO_DIR)/parlando-agent-sdk
PYTHON_TEST_VENV := $(PARLANDO_DIR)/.local/python-test-venv
PYTHON_TEST := $(PYTHON_TEST_VENV)/bin/python
NPM_CACHE ?= $(PARLANDO_DIR)/.local/npm-cache
PYTHON ?= python3

.PHONY: all test test-rust test-js-client test-client-server test-browser-e2e test-prolific-e2e test-e2e test-e2e-logging test-python install-local install-js-client package-local package-rust-server-local package-js-client-local publish-dry-run publish-rust-server-dry-run publish-js-client-dry-run publish-rust-server publish-js-client

# Default workflow: prepare the reusable JavaScript client for local development.
all: install-local

# Runs the complete reusable-server, browser-client, and Python-SDK test matrix.
test: test-rust test-client-server test-python

# Runs the Rust unit, integration, protocol, and documentation tests.
test-rust:
	cd "$(RUST_SERVER_DIR)" && cargo test --all-features
	cd "$(RUST_SERVER_TESTS_DIR)" && cargo test

# Runs browser-client tests, type compilation, and the coverage regression gate.
test-js-client:
	cd "$(JS_CLIENT_DIR)" && npm --cache "$(NPM_CACHE)" test
	cd "$(JS_CLIENT_DIR)" && npm --cache "$(NPM_CACHE)" run build
	cd "$(JS_CLIENT_DIR)" && npm --cache "$(NPM_CACHE)" run test:coverage
	cd "$(JS_CLIENT_DIR)" && npm --cache "$(NPM_CACHE)" run test:package

# Runs the explicit live client/server contract suite, including its Node audio driver.
test-client-server: test-js-client
	cd "$(CLIENT_SERVER_TESTS_DIR)" && cargo test

# Builds the disposable browser client and runs the real Chromium lifecycle matrix.
test-browser-e2e: test-js-client
	cd "$(CLIENT_SERVER_TESTS_DIR)/browser" && npm --cache "$(NPM_CACHE)" install
	cd "$(CLIENT_SERVER_TESTS_DIR)/browser" && npm --cache "$(NPM_CACHE)" run install:browsers
	cd "$(CLIENT_SERVER_TESTS_DIR)/browser" && npm --cache "$(NPM_CACHE)" run build
	cd "$(CLIENT_SERVER_TESTS_DIR)" && cargo test --test browser_e2e -- --ignored --nocapture

# Runs the standalone process-boundary Prolific scenario matrix and its durable report.
test-prolific-e2e:
	cd "$(RUST_SERVER_TESTS_DIR)" && cargo build --bins
	cd "$(RUST_SERVER_TESTS_DIR)" && cargo run --bin prolific-test-runner

# Release-oriented acceptance gate. The coordinator runs every layer after failures.
test-e2e: test-e2e-logging
	@set +e; set -o pipefail; \
	mkdir -p "$(PARLANDO_DIR)/target/e2e-runs" || exit 1; \
	run_dir=$$(mktemp -d "$(PARLANDO_DIR)/target/e2e-runs/$$(date +%Y%m%d-%H%M%S)-XXXXXX") || exit 1; \
	printf 'E2E logs: %s\n' "$$run_dir"; \
	$(MAKE) test-js-client 2>&1 | tee "$$run_dir/javascript.log"; \
	js_status=$$?; \
	cargo test --manifest-path "$(PARLANDO_DIR)/rust-server/Cargo.toml" 2>&1 | tee "$$run_dir/rust.log"; \
	rust_status=$$?; \
	(cd "$(CLIENT_SERVER_TESTS_DIR)" && cargo test) 2>&1 | tee "$$run_dir/contracts.log"; \
	contract_status=$$?; \
	(cd "$(CLIENT_SERVER_TESTS_DIR)/browser" && npm --cache "$(NPM_CACHE)" install && npm --cache "$(NPM_CACHE)" run install:browsers && npm --cache "$(NPM_CACHE)" run build && cd "$(CLIENT_SERVER_TESTS_DIR)" && cargo test --test browser_e2e -- --ignored --nocapture) 2>&1 | tee "$$run_dir/browser.log"; \
	browser_status=$$?; \
	if [ $$browser_status -eq 0 ]; then cp "$(CLIENT_SERVER_TESTS_DIR)/target/browser-e2e/report.md" "$$run_dir/browser-report.md" || exit 1; fi; \
	(cd "$(RUST_SERVER_TESTS_DIR)" && cargo build --bins && cargo run --bin prolific-test-runner) 2>&1 | tee "$$run_dir/prolific.log"; \
	prolific_status=$$?; \
	"$(PYTHON)" "$(PARLANDO_DIR)/scripts/write_e2e_report.py" "$$run_dir" $$js_status $$rust_status $$contract_status $$browser_status $$prolific_status || exit 1; \
	if [ $$js_status -ne 0 ] || [ $$rust_status -ne 0 ] || [ $$contract_status -ne 0 ] || [ $$browser_status -ne 0 ] || [ $$prolific_status -ne 0 ]; then exit 1; fi

# Verify that the coordinator preserves failures and complete layer output.
test-e2e-logging:
	"$(PYTHON)" -m unittest discover -s "$(PARLANDO_DIR)/scripts" -p test_e2e_logging.py -v

# Runs the Python SDK suite in an environment where its package dependencies are installed.
test-python:
	"$(PYTHON)" -m venv "$(PYTHON_TEST_VENV)"
	"$(PYTHON_TEST)" -m pip install --quiet --disable-pip-version-check --editable "$(PYTHON_AGENT_SDK_DIR)"
	cd "$(PYTHON_AGENT_SDK_DIR)" && "$(PYTHON_TEST)" -m unittest discover -s tests -v

# Install all top-level local dependencies.
install-local: install-js-client

# Install JavaScript client dependencies and build the local package output.
install-js-client:
	cd "$(JS_CLIENT_DIR)" && npm --cache "$(NPM_CACHE)" install
	cd "$(JS_CLIENT_DIR)" && npm --cache "$(NPM_CACHE)" run build

# Prepare local Rust and JavaScript packages without publishing to remote registries.
package-local: package-rust-server-local package-js-client-local

# Create a local Cargo package for the Rust server, allowing uncommitted changes.
package-rust-server-local:
	cd "$(RUST_SERVER_DIR)" && cargo package --allow-dirty

# Verify the JavaScript client package shape without publishing it.
package-js-client-local:
	cd "$(JS_CLIENT_DIR)" && npm --cache "$(NPM_CACHE)" pack --dry-run

# Run publishing checks without uploading packages.
publish-dry-run: publish-rust-server-dry-run publish-js-client-dry-run

# Validate the Rust server crate against Cargo's publish checks without uploading it.
publish-rust-server-dry-run:
	cd "$(RUST_SERVER_DIR)" && cargo publish --dry-run --allow-dirty

# Validate the JavaScript client package against npm's publish checks without uploading it.
publish-js-client-dry-run:
	cd "$(JS_CLIENT_DIR)" && npm --cache "$(NPM_CACHE)" publish --dry-run

publish: publish-rust-server publish-js-client

# Publish the Rust server crate to the configured Cargo registry.
publish-rust-server:
	cd "$(RUST_SERVER_DIR)" && cargo publish

# Publish the JavaScript client package to the configured npm registry.
publish-js-client:
	cd "$(JS_CLIENT_DIR)" && npm --cache "$(NPM_CACHE)" publish
