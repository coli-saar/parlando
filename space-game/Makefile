SHELL := /bin/bash

SPACE_GAME_DIR := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
PARLANDO_DIR := $(abspath $(SPACE_GAME_DIR)/..)
CLIENT_DIR := $(SPACE_GAME_DIR)/client
JS_CLIENT_DIR := $(PARLANDO_DIR)/js-client
LOCAL_PREFIX ?= $(PARLANDO_DIR)/.local
LOCAL_BIN := $(LOCAL_PREFIX)/bin
NPM_CACHE ?= $(PARLANDO_DIR)/.local/npm-cache

HOST ?= 127.0.0.1
PORT ?= 8000
DATABASE_URL ?= sqlite:///$(SPACE_GAME_DIR)/.local/parlando.sqlite
SERVER_BIN ?= $(LOCAL_BIN)/parlando-space-game
CLIENT_PACKAGE ?= @coli-saar/parlando-client
LOCAL_CLIENT_PACKAGE := $(JS_CLIENT_DIR)

.PHONY: all install-local install-server install-server-local prepare-client-local check-server check-client-package install-client-deps install-client-deps-local build-client build-client-local build build-local run run-local clean-client clean

# Default workflow: build the Space Game browser client and verify the server binary.
all: build

# Build and install the Space Game server binary from published Rust dependencies.
install-server:
	cd "$(PARLANDO_DIR)" && cargo install --path space-game/server --root "$(LOCAL_PREFIX)" --force

# Build and install the Space Game server binary with the sibling Parlando crate patched in.
install-server-local:
	cd "$(PARLANDO_DIR)" && cargo install --path space-game/server --root "$(LOCAL_PREFIX)" --force --config 'patch.crates-io.parlando.path="$(PARLANDO_DIR)/rust-server"'

# Preserve the existing local preparation target for callers that invoke it directly.
install-local: install-server-local prepare-client-local

# Build the shared JavaScript client package used by the Space Game client.
prepare-client-local:
	cd "$(JS_CLIENT_DIR)" && npm --cache "$(NPM_CACHE)" install
	cd "$(JS_CLIENT_DIR)" && npm --cache "$(NPM_CACHE)" run build

# Verify that the configured Space Game server binary is available.
check-server:
	@command -v "$(SERVER_BIN)" >/dev/null || { \
		echo "Missing $(SERVER_BIN). Run 'make install-server' from this directory, or set SERVER_BIN=/path/to/parlando-space-game."; \
		exit 1; \
	}

# Verify that the shared JavaScript client checkout has been built for a local override.
check-client-package:
	@test -f "$(LOCAL_CLIENT_PACKAGE)/package.json" && test -f "$(LOCAL_CLIENT_PACKAGE)/dist/index.js" || { \
		echo "Missing built local package $(CLIENT_PACKAGE). Run 'make prepare-client-local' from this directory."; \
		exit 1; \
	}

# Install the Space Game client dependencies from the published SDK manifest.
install-client-deps:
	cd "$(CLIENT_DIR)" && npm --cache "$(NPM_CACHE)" install

# Install published client dependencies, then replace only the SDK in node_modules locally.
install-client-deps-local: check-client-package
	cd "$(CLIENT_DIR)" && npm --cache "$(NPM_CACHE)" install
	cd "$(CLIENT_DIR)" && npm --cache "$(NPM_CACHE)" install --no-save --package-lock=false "$(LOCAL_CLIENT_PACKAGE)"

# Build the Space Game browser client.
build-client: install-client-deps
	cd "$(CLIENT_DIR)" && npm --cache "$(NPM_CACHE)" run build

# Build the browser client using the sibling SDK checkout.
build-client-local: prepare-client-local install-client-deps-local
	cd "$(CLIENT_DIR)" && npm --cache "$(NPM_CACHE)" run build

# Prepare published dependencies, build the browser client, and verify the server binary.
build: install-server build-client check-server

# Prepare sibling Parlando dependencies, build the browser client, and verify the server binary.
build-local: install-server-local build-client-local check-server

# Run the Space Game with published Parlando dependencies.
run: build
	mkdir -p "$(SPACE_GAME_DIR)/.local"
	"$(SERVER_BIN)" --host "$(HOST)" --port "$(PORT)" --database-url "$(DATABASE_URL)" --client-dist "$(CLIENT_DIR)/dist"

# Run the Space Game with the sibling Rust server and JavaScript client checkouts.
run-local: build-local
	mkdir -p "$(SPACE_GAME_DIR)/.local"
	"$(SERVER_BIN)" --host "$(HOST)" --port "$(PORT)" --database-url "$(DATABASE_URL)" --client-dist "$(CLIENT_DIR)/dist"

# Remove generated browser-client dependencies and build output.
clean-client:
	cd "$(CLIENT_DIR)" && rm -rf dist node_modules package-lock.json

# Remove generated Space Game artifacts.
clean: clean-client
