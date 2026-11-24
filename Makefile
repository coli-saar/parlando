SHELL := /bin/bash

PARLANDO_DIR := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))

RUST_SERVER_DIR := $(PARLANDO_DIR)/rust-server
RUST_SERVER_TESTS_DIR := $(PARLANDO_DIR)/rust-server-tests
COMPONENT_TEST_DIR := $(PARLANDO_DIR)/target/component-tests
STRESS_FIXTURE_DIR := $(RUST_SERVER_TESTS_DIR)/stress-fixture
CLIENT_SERVER_TESTS_DIR := $(PARLANDO_DIR)/client-server-tests
JS_CLIENT_DIR := $(PARLANDO_DIR)/js-client
RUST_GAME_SDK_DIR := $(PARLANDO_DIR)/parlando-game-sdk
PYTHON_AGENT_SDK_DIR := $(PARLANDO_DIR)/parlando-agent-sdk
PYTHON_TEST_VENV := $(PARLANDO_DIR)/.local/python-test-venv
PYTHON_TEST := $(PYTHON_TEST_VENV)/bin/python
NPM_CACHE ?= $(PARLANDO_DIR)/.local/npm-cache
PYTHON ?= python3
RELEASE_VENV := $(PARLANDO_DIR)/.local/release-venv
RELEASE_PYTHON ?= $(if $(filter command line environment,$(origin PYTHON)),$(PYTHON),$(if $(wildcard $(RELEASE_VENV)/bin/python),$(RELEASE_VENV)/bin/python,$(PYTHON)))
RELEASE_DIST := $(PARLANDO_DIR)/target/release-packages
RELEASE_VERSION = $(shell node -p 'require("$(JS_CLIENT_DIR)/package.json").version')
CLIENT_TARBALL = $(RELEASE_DIST)/coli-saar-parlando-client-$(RELEASE_VERSION).tgz
RELEASE_REHEARSAL := $(PARLANDO_DIR)/scripts/test_release_candidate.py

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
	"$(PYTHON)" -m unittest discover -s "$(PARLANDO_DIR)/scripts" -p 'test_*logging.py' -v

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
	"$(RELEASE_PYTHON)" "$(PARLANDO_DIR)/scripts/release_crate.py" "$(RUST_GAME_SDK_DIR)/derive/Cargo.toml"

publish-rust-sdk-dry-run:
	"$(RELEASE_PYTHON)" "$(PARLANDO_DIR)/scripts/release_crate.py" "$(RUST_GAME_SDK_DIR)/Cargo.toml"

publish-rust-server-dry-run:
	"$(RELEASE_PYTHON)" "$(PARLANDO_DIR)/scripts/release_crate.py" "$(RUST_SERVER_DIR)/Cargo.toml"

# The release reporter invokes each named Make target and retains its full output.
RELEASE_MAKE := $(MAKE)
RELEASE_REPORT := "$(RELEASE_PYTHON)" "$(PARLANDO_DIR)/scripts/release_output.py"
.PHONY: release-preflight release-server-build release-manual release-rehearsal
release-preflight: check-release-versions check-release-tools

test-release-candidate:
	@PYTHON="$(RELEASE_PYTHON)" $(RELEASE_REPORT) pre-release "$(RELEASE_MAKE)" \
	  --step "Validate release configuration" release-preflight \
	  --step "Build packages and check registry dry runs" publish-dry-run \
	  --step "Unit and client/server tests" test \
	  --step "Browser and Prolific scenarios" test-e2e \
	  --step "Component compatibility" test-components \
	  --step "Communication stress matrix" test-runtime-communication \
	  --step "Build optimized server" release-server-build \
	  --step "Build documentation" release-manual \
	  --step "Test packaged games and agents" release-rehearsal

release-server-build:
	cargo build --locked --release --all-features --manifest-path "$(RUST_SERVER_DIR)/Cargo.toml"

release-manual:
	hugo --source "$(PARLANDO_DIR)/docs/web" --destination "$(PARLANDO_DIR)/target/release-manual" --panicOnWarning --printPathWarnings

release-rehearsal:
	npm_config_cache="$(NPM_CACHE)" "$(RELEASE_PYTHON)" "$(RELEASE_REHEARSAL)" --packages "$(RELEASE_DIST)"

publish:
	@$(RELEASE_REPORT) publish "$(RELEASE_MAKE)" \
	  --step "Validate release configuration" release-preflight \
	  --step "Publish parlando-game-derive" publish-rust-derive \
	  --step "Publish parlando-game-sdk" publish-rust-sdk \
	  --step "Publish parlando" publish-rust-server \
	  --step "Publish @coli-saar/parlando-client" publish-js-client \
	  --step "Publish parlando-python-sdk" publish-python-game-sdk \
	  --step "Publish parlando-agent-sdk" publish-python-agent-sdk

publish-rust:
	$(MAKE) publish-rust-derive
	$(MAKE) publish-rust-sdk
	$(MAKE) publish-rust-server

# Require every package to carry the coordinated release version.
check-release-versions:
	"$(RELEASE_PYTHON)" "$(PARLANDO_DIR)/skills/release-parlando/scripts/check_release.py" --packages-only

# Verify the installed release tools before packaging or uploading.
check-release-tools:
	"$(RELEASE_PYTHON)" -m build --version
	"$(RELEASE_PYTHON)" -m twine --version

publish-rust-derive:
	cd "$(RUST_GAME_SDK_DIR)/derive" && $(REGISTRY_RUN) cargo publish --registry crates-io

publish-rust-sdk:
	cd "$(RUST_GAME_SDK_DIR)" && $(REGISTRY_RUN) cargo publish --registry crates-io

publish-rust-server:
	cd "$(RUST_SERVER_DIR)" && $(REGISTRY_RUN) cargo publish --registry crates-io

# Both release workflows rebuild their packages. Stage only the package inputs in a fresh directory.
.PHONY: pack-js-client pack-python-game-sdk pack-python-agent-sdk
pack-js-client:
	@mkdir -p "$(RELEASE_DIST)"
	@set -eu; stage=$$(mktemp -d "$(RELEASE_DIST)/client.XXXXXX"); trap 'rm -rf "$$stage"' EXIT; \
	cp -R "$(JS_CLIENT_DIR)"/{package.json,package-lock.json,tsconfig.json,README.md,src} "$$stage/"; \
	cd "$$stage"; npm --cache "$(NPM_CACHE)" ci --include=dev --no-audit --no-fund; \
	npm --cache "$(NPM_CACHE)" pack; mv ./*.tgz "$(RELEASE_DIST)/"

pack-python-game-sdk: SDK := parlando-python-sdk
pack-python-agent-sdk: SDK := parlando-agent-sdk
pack-python-game-sdk pack-python-agent-sdk: check-release-tools
	@mkdir -p "$(RELEASE_DIST)"
	@set -eu; stage=$$(mktemp -d "$(RELEASE_DIST)/python.XXXXXX"); trap 'rm -rf "$$stage"' EXIT; \
	cp -R "$(PARLANDO_DIR)/$(SDK)"/{pyproject.toml,README.md,src,tests} "$$stage/"; \
	"$(RELEASE_PYTHON)" -m build "$$stage"; \
	"$(RELEASE_PYTHON)" -m twine check --strict "$$stage"/dist/*; \
	mv "$$stage"/dist/* "$(RELEASE_DIST)/"

publish-js-client publish-js-client-dry-run: pack-js-client
publish-js-client-dry-run: NPM_RELEASE_FLAGS := --dry-run
publish-js-client publish-js-client-dry-run:
	cd "$(JS_CLIENT_DIR)" && $(REGISTRY_RUN) npm --cache "$(NPM_CACHE)" publish "$(CLIENT_TARBALL)" --access public $(NPM_RELEASE_FLAGS)

publish-python-game-sdk publish-python-game-sdk-dry-run: pack-python-game-sdk
publish-python-agent-sdk publish-python-agent-sdk-dry-run: pack-python-agent-sdk
publish-python-game-sdk: SDK_DIST := parlando_python_sdk
publish-python-agent-sdk: SDK_DIST := parlando_agent_sdk
publish-python-game-sdk publish-python-agent-sdk:
	$(REGISTRY_RUN) "$(RELEASE_PYTHON)" -m twine upload --repository pypi "$(RELEASE_DIST)/$(SDK_DIST)-$(RELEASE_VERSION)"{.tar.gz,-py3-none-any.whl}
