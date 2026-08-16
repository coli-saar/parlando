SHELL := /bin/bash

PARLANDO_DIR := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))

RUST_SERVER_DIR := $(PARLANDO_DIR)/rust-server
RUST_SERVER_TESTS_DIR := $(PARLANDO_DIR)/rust-server-tests
JS_CLIENT_DIR := $(PARLANDO_DIR)/js-client
NPM_CACHE ?= $(PARLANDO_DIR)/.local/npm-cache

.PHONY: all test test-rust test-js-client test-client-server test-python install-local install-js-client package-local package-rust-server-local package-js-client-local publish-dry-run publish-rust-server-dry-run publish-js-client-dry-run publish-rust-server publish-js-client

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

# Runs the built JavaScript audio sink against the production Rust audio WebSocket.
test-client-server: test-js-client
	cd "$(RUST_SERVER_DIR)" && cargo test app::tests::javascript_sink_mute_contract_blocks_relay_and_transcription --lib -- --ignored --exact

# Runs the Python SDK suite in an environment where its package dependencies are installed.
test-python:
	cd "$(RUST_SERVER_DIR)/python/parlando-agent-sdk" && python3 -m unittest discover -s tests -v

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
