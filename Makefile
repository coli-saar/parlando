SHELL := /bin/bash

PARLANDO_DIR := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))

RUST_SERVER_DIR := $(PARLANDO_DIR)/rust-server
RUST_SERVER_TESTS_DIR := $(PARLANDO_DIR)/rust-server-tests
COMPONENT_TEST_DIR := $(PARLANDO_DIR)/target/component-tests
STRESS_FIXTURE_DIR := $(RUST_SERVER_TESTS_DIR)/stress-fixture
CLIENT_SERVER_TESTS_DIR := $(PARLANDO_DIR)/client-server-tests
JS_CLIENT_DIR := $(PARLANDO_DIR)/js-client
RUST_GAME_SDK_DIR := $(PARLANDO_DIR)/parlando-game-sdk
PYTHON_GAME_SDK_DIR := $(PARLANDO_DIR)/parlando-python-sdk
PYTHON_AGENT_SDK_DIR := $(PARLANDO_DIR)/parlando-agent-sdk
PYTHON_TEST_VENV := $(PARLANDO_DIR)/.local/python-test-venv
PYTHON_TEST := $(PYTHON_TEST_VENV)/bin/python
NPM_CACHE ?= $(PARLANDO_DIR)/.local/npm-cache
PYTHON ?= python3
RELEASE_CANDIDATES := $(PARLANDO_DIR)/target/release-candidates
RELEASE_REHEARSAL := $(PARLANDO_DIR)/scripts/test_release_candidate.py
verified_release = "$(PYTHON)" "$(RELEASE_REHEARSAL)" --verified "$(RELEASE_CANDIDATES)"

.PHONY: all test test-rust test-components test-js-client test-client-server test-browser-e2e test-prolific-e2e test-e2e test-e2e-logging test-python install-local install-js-client package-local package-rust-server-local package-js-client-local

# Default workflow: prepare the reusable JavaScript client for local development.
all: install-local

.PHONY: sync-brand
# Refresh packaged copies of the canonical logos and screen palette.
sync-brand:
	cp brand/parlando-mark.svg brand/parlando-wordmark.svg brand/palette.css rust-server/brand/
	cp brand/parlando-mark.svg brand/parlando-wordmark.svg brand/palette.css docs/web/static/brand/
	cp brand/parlando-mark.svg js-client/src/participant/

# Runs the complete reusable-server, browser-client, and Python-SDK test matrix.
test: test-rust test-client-server test-python

.PHONY: test-runtime-communication
# Plays concurrent fixture games in each communication mode and pairing, stopping on failure.
test-runtime-communication:
	@set -e; for mode in typed speech; do \
	  for pairing in human-human human-agent; do \
	    cargo stress --communication "$$mode" --pairing "$$pairing" \
	      --sessions 4 --seconds 20 --headless; \
	  done; \
	done

# Runs the Rust unit, integration, protocol, and documentation tests.
test-rust:
	cd "$(RUST_SERVER_DIR)" && cargo test --all-features
	cd "$(RUST_SERVER_TESTS_DIR)" && cargo test

# Builds the guest fixtures and runs the component compatibility suite.
test-components:
	cmp "$(RUST_GAME_SDK_DIR)/wit/agent/agent.wit" "$(RUST_SERVER_DIR)/wit/agent/agent.wit"
	cargo test --locked --manifest-path "$(RUST_GAME_SDK_DIR)/Cargo.toml"
	cargo test --locked --manifest-path "$(STRESS_FIXTURE_DIR)/Cargo.toml"
	mkdir -p "$(COMPONENT_TEST_DIR)"
	cargo build --locked --release --manifest-path "$(STRESS_FIXTURE_DIR)/Cargo.toml" \
	  --target wasm32-unknown-unknown --target-dir "$(COMPONENT_TEST_DIR)/cargo" --features test-traps --lib --example sdk-encoding-trap
	wasm-tools component new "$(COMPONENT_TEST_DIR)/cargo/wasm32-unknown-unknown/release/parlando_stress_fixture.wasm" \
	  -o "$(COMPONENT_TEST_DIR)/stress.wasm"
	wasm-tools component new "$(COMPONENT_TEST_DIR)/cargo/wasm32-unknown-unknown/release/examples/sdk_encoding_trap.wasm" \
	  -o "$(COMPONENT_TEST_DIR)/sdk-encoding-trap.wasm"
	wasm-tools component embed --world game "$(RUST_SERVER_DIR)/wit" "$(STRESS_FIXTURE_DIR)/independent.wat" \
	  -o "$(COMPONENT_TEST_DIR)/independent-core.wasm"
	wasm-tools component new "$(COMPONENT_TEST_DIR)/independent-core.wasm" -o "$(COMPONENT_TEST_DIR)/independent.wasm"
	cargo test --locked --manifest-path "$(RUST_SERVER_DIR)/Cargo.toml" \
	  --lib --bin parlando
	PARLANDO_TEST_COMPONENT="$(COMPONENT_TEST_DIR)/stress.wasm" \
	  PARLANDO_TEST_WAT_COMPONENT="$(COMPONENT_TEST_DIR)/independent.wasm" \
	  PARLANDO_TEST_SDK_COMPONENT="$(COMPONENT_TEST_DIR)/sdk-encoding-trap.wasm" \
	  cargo test --locked --manifest-path "$(RUST_SERVER_TESTS_DIR)/Cargo.toml" \
	  --features stress-tui --bin runtime-stress -- --include-ignored

# Runs participant and dashboard tests, type compilation, and the coverage regression gate.
test-js-client:
	cd "$(RUST_SERVER_DIR)" && node --test tests/admin_dashboard*.test.mjs
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
	cd "$(CLIENT_SERVER_TESTS_DIR)/browser" && npm --cache "$(NPM_CACHE)" test
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
	(cd "$(CLIENT_SERVER_TESTS_DIR)/browser" && npm --cache "$(NPM_CACHE)" install && npm --cache "$(NPM_CACHE)" run install:browsers && npm --cache "$(NPM_CACHE)" run build && npm --cache "$(NPM_CACHE)" test && cd "$(CLIENT_SERVER_TESTS_DIR)" && cargo test --test browser_e2e -- --ignored --nocapture) 2>&1 | tee "$$run_dir/browser.log"; \
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

# Check every registry artifact before publication.
.PHONY: publish publish-dry-run publish-rust publish-rust-derive publish-rust-sdk \
        publish-rust-server publish-js-client publish-rust-server-dry-run publish-js-client-dry-run \
        publish-python-game-sdk publish-python-agent-sdk \
        publish-rust-derive-dry-run publish-rust-sdk-dry-run \
        publish-python-game-sdk-dry-run publish-python-agent-sdk-dry-run check-release-tools check-release-versions \
        test-release-candidate
publish-dry-run: check-release-versions publish-rust-derive-dry-run publish-rust-sdk-dry-run \
                 publish-rust-server-dry-run publish-js-client-dry-run \
                 publish-python-game-sdk-dry-run publish-python-agent-sdk-dry-run

publish-rust-derive-dry-run:
	"$(PYTHON)" "$(PARLANDO_DIR)/scripts/release_crate.py" "$(RUST_GAME_SDK_DIR)/derive/Cargo.toml"

publish-rust-sdk-dry-run:
	"$(PYTHON)" "$(PARLANDO_DIR)/scripts/release_crate.py" "$(RUST_GAME_SDK_DIR)/Cargo.toml"

publish-rust-server-dry-run:
	"$(PYTHON)" "$(PARLANDO_DIR)/scripts/release_crate.py" "$(RUST_SERVER_DIR)/Cargo.toml"

publish-js-client-dry-run:
	cd "$(JS_CLIENT_DIR)" && npm --cache "$(NPM_CACHE)" publish --dry-run

publish-python-game-sdk-dry-run publish-python-agent-sdk-dry-run:
	bash "$(PARLANDO_DIR)/scripts/release-python-package.sh" "$(PYTHON)" "$(PYTHON_RELEASE_SOURCE)"

# Run the full test matrix before saving a candidate for publication.
test-release-candidate: check-release-versions check-release-tools
	$(MAKE) test
	$(MAKE) test-e2e
	$(MAKE) test-components
	$(MAKE) test-runtime-communication
	npm_config_cache="$(NPM_CACHE)" "$(PYTHON)" "$(RELEASE_REHEARSAL)" --output "$(RELEASE_CANDIDATES)"

# Serial sub-makes preserve publication order and stop on a failed upload even with make -j.
publish: check-release-versions check-release-tools
	@set -e; bundle=$$($(verified_release)); \
	$(MAKE) publish-rust; \
	$(MAKE) publish-js-client RELEASE_BUNDLE="$$bundle"; \
	$(MAKE) publish-python-game-sdk RELEASE_BUNDLE="$$bundle"; \
	$(MAKE) publish-python-agent-sdk RELEASE_BUNDLE="$$bundle"

publish-rust:
	$(MAKE) publish-rust-derive
	$(MAKE) publish-rust-sdk
	$(MAKE) publish-rust-server

# Require every package to carry the coordinated release version.
check-release-versions:
	"$(PYTHON)" "$(PARLANDO_DIR)/skills/release-parlando/scripts/check_release.py" --packages-only

# Check the Python upload tools before starting uploads to any registry.
check-release-tools:
	"$(PYTHON)" -m build --version
	"$(PYTHON)" -m twine --version

publish-rust-derive:
	cd "$(RUST_GAME_SDK_DIR)/derive" && cargo publish --registry crates-io

publish-rust-sdk:
	cd "$(RUST_GAME_SDK_DIR)" && cargo publish --registry crates-io

publish-rust-server:
	cd "$(RUST_SERVER_DIR)" && cargo publish --registry crates-io

publish-js-client:
	@set -e; bundle="$(RELEASE_BUNDLE)"; \
	if [ -z "$$bundle" ]; then bundle=$$($(verified_release)); fi; \
	cd "$(JS_CLIENT_DIR)" && npm --cache "$(NPM_CACHE)" publish "$$bundle"/coli-saar-parlando-client-*.tgz --access public

# Build from the corresponding source tree for each Python dry run.
publish-python-game-sdk-dry-run: PYTHON_RELEASE_SOURCE := $(PYTHON_GAME_SDK_DIR)
publish-python-agent-sdk-dry-run: PYTHON_RELEASE_SOURCE := $(PYTHON_AGENT_SDK_DIR)

publish-python-game-sdk: PYTHON_ARTIFACT := parlando_python_sdk
publish-python-agent-sdk: PYTHON_ARTIFACT := parlando_agent_sdk
publish-python-game-sdk publish-python-agent-sdk:
	@set -e; bundle="$(RELEASE_BUNDLE)"; \
	if [ -z "$$bundle" ]; then bundle=$$($(verified_release)); fi; \
	"$(PYTHON)" -m twine upload --repository pypi "$$bundle"/$(PYTHON_ARTIFACT)-*.tar.gz "$$bundle"/$(PYTHON_ARTIFACT)-*.whl
