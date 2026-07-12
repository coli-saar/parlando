SHELL := /bin/bash

SPACE_GAME_DIR := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
PARLANDO_DIR := $(abspath $(SPACE_GAME_DIR)/..)
CLIENT_DIR := $(SPACE_GAME_DIR)/client
JS_CLIENT_DIR := $(PARLANDO_DIR)/js-client
LOCAL_PREFIX ?= $(PARLANDO_DIR)/.local
LOCAL_BIN := $(LOCAL_PREFIX)/bin

HOST ?= 127.0.0.1
PORT ?= 8000
CONFIG ?= $(SPACE_GAME_DIR)/config/experiment.local-rust.yaml
SERVER_BIN ?= $(LOCAL_BIN)/parlando-space-game
CLIENT_PACKAGE ?= @parlando/client
CLIENT_VERSION ?= 0.1.0
YALC_STORE := $(shell yalc dir 2>/dev/null)
LOCAL_CLIENT_PACKAGE := $(YALC_STORE)/packages/@parlando/client/$(CLIENT_VERSION)

.PHONY: all install-local install-server package-client-local check-server check-client-package install-client-deps build-client build run run-solo-voice clean-client clean

# Default workflow: build the Space Game browser client and verify the server binary.
all: build

# Install the Space Game server binary and prepare the shared browser client locally.
install-local: install-server package-client-local

# Build and install the Space Game Rust server binary into the repository-local .local/bin.
install-server:
	cd "$(PARLANDO_DIR)" && cargo install --path space-game/server --root "$(LOCAL_PREFIX)" --force

# Install and yalc-link the shared JavaScript client package used by the Space Game client.
package-client-local:
	cd "$(JS_CLIENT_DIR)" && npm install
	cd "$(JS_CLIENT_DIR)" && npm run yalc

# Verify that the configured Space Game server binary is available.
check-server:
	@command -v "$(SERVER_BIN)" >/dev/null || { \
		echo "Missing $(SERVER_BIN). Run 'make install-server' from this directory, or set SERVER_BIN=/path/to/parlando-space-game."; \
		exit 1; \
	}

# Verify that the shared JavaScript client is available in the local yalc store.
check-client-package:
	@test -f "$(LOCAL_CLIENT_PACKAGE)/package.json" || { \
		echo "Missing local yalc package $(CLIENT_PACKAGE)@$(CLIENT_VERSION). Run 'make package-client-local' from this directory."; \
		exit 1; \
	}

# Install the local yalc copy of @parlando/client into the Space Game browser app.
install-client-deps: check-client-package
	cd "$(CLIENT_DIR)" && npm install --no-save "$(LOCAL_CLIENT_PACKAGE)"

# Build the Space Game browser client.
build-client: install-client-deps
	cd "$(CLIENT_DIR)" && npm run build

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
	cd "$(CLIENT_DIR)" && rm -rf dist node_modules .yalc yalc.lock package-lock.json

# Remove generated Space Game artifacts.
clean: clean-client
