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
CONFIG ?= $(SPACE_GAME_DIR)/config/experiment.local-rust.yaml
SERVER_BIN ?= $(LOCAL_BIN)/parlando-space-game
CLIENT_PACKAGE ?= @coli-saar/parlando-client
LOCAL_CLIENT_PACKAGE := $(JS_CLIENT_DIR)

.PHONY: all install-local install-server prepare-client-local check-server check-client-package install-client-deps build-client build run run-solo-voice clean-client clean

# Default workflow: build the Space Game browser client and verify the server binary.
all: build

# Install the Space Game server binary and prepare the shared browser client locally.
install-local: install-server prepare-client-local

# Build and install the Space Game Rust server binary into the repository-local .local/bin.
install-server:
	cd "$(PARLANDO_DIR)" && cargo install --path space-game/server --root "$(LOCAL_PREFIX)" --force

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

# Verify that the shared JavaScript client checkout has been built.
check-client-package:
	@test -f "$(LOCAL_CLIENT_PACKAGE)/package.json" && test -f "$(LOCAL_CLIENT_PACKAGE)/dist/index.js" || { \
		echo "Missing built local package $(CLIENT_PACKAGE). Run 'make prepare-client-local' from this directory."; \
		exit 1; \
	}

# Install the Space Game client dependencies, including the manifest's local SDK path.
install-client-deps: check-client-package
	cd "$(CLIENT_DIR)" && npm --cache "$(NPM_CACHE)" install

# Build the Space Game browser client.
build-client: install-client-deps
	cd "$(CLIENT_DIR)" && npm --cache "$(NPM_CACHE)" run build

# Prepare local dependencies, build the browser client, and verify the server binary.
build: install-local build-client check-server

# Run the Space Game server/client stack; override HOST, PORT, CONFIG, or SERVER_BIN as needed.
run: build
	mkdir -p "$(SPACE_GAME_DIR)/.local"
	"$(SERVER_BIN)" --host "$(HOST)" --port "$(PORT)" --config "$(CONFIG)"

# Run the Space Game in the solo voice configuration.
run-solo-voice:
	$(MAKE) run CONFIG="$(SPACE_GAME_DIR)/config/experiment.local-back-and-forth-voice.yaml"

# Remove generated browser-client dependencies and build output.
clean-client:
	cd "$(CLIENT_DIR)" && rm -rf dist node_modules package-lock.json

# Remove generated Space Game artifacts.
clean: clean-client
