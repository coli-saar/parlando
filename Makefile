SHELL := /bin/bash

PARLANDO_DIR := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))

RUST_SERVER_DIR := $(PARLANDO_DIR)/rust-server
JS_CLIENT_DIR := $(PARLANDO_DIR)/js-client

.PHONY: all install-local install-js-client package-local package-rust-server-local package-js-client-local publish-dry-run publish-rust-server-dry-run publish-js-client-dry-run publish-rust-server publish-js-client

# Default workflow: prepare the reusable JavaScript client for local development.
all: install-local

# Install all top-level local dependencies.
install-local: install-js-client

# Install JavaScript client dependencies and link the client locally with yalc.
install-js-client:
	cd "$(JS_CLIENT_DIR)" && npm install
	cd "$(JS_CLIENT_DIR)" && npm run yalc

# Prepare local Rust and JavaScript packages without publishing to remote registries.
package-local: package-rust-server-local package-js-client-local

# Create a local Cargo package for the Rust server, allowing uncommitted changes.
package-rust-server-local:
	cd "$(RUST_SERVER_DIR)" && cargo package --allow-dirty

# Package/link the JavaScript client into the local yalc store.
package-js-client-local:
	cd "$(JS_CLIENT_DIR)" && npm run yalc

# Run publishing checks without uploading packages.
publish-dry-run: publish-rust-server-dry-run publish-js-client-dry-run

# Validate the Rust server crate against Cargo's publish checks without uploading it.
publish-rust-server-dry-run:
	cd "$(RUST_SERVER_DIR)" && cargo publish --dry-run --allow-dirty

# Validate the JavaScript client package against npm's publish checks without uploading it.
publish-js-client-dry-run:
	cd "$(JS_CLIENT_DIR)" && npm publish --dry-run

# Publish the Rust server crate to the configured Cargo registry.
publish-rust-server:
	cd "$(RUST_SERVER_DIR)" && cargo publish

# Publish the JavaScript client package to the configured npm registry.
publish-js-client:
	cd "$(JS_CLIENT_DIR)" && npm publish
