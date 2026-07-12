SHELL := /bin/bash

SPACE_GAME_DIR := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))

HOST ?= 127.0.0.1
PORT ?= 8000
CONFIG ?= $(SPACE_GAME_DIR)/config/experiment.local-rust.yaml
SERVER_BIN ?= parlando-space-game
CLIENT_PACKAGE ?= @parlando/client
CLIENT_VERSION ?= 0.1.0
YALC_STORE := $(shell yalc dir 2>/dev/null)
LOCAL_CLIENT_PACKAGE := $(YALC_STORE)/packages/@parlando/client/$(CLIENT_VERSION)

.PHONY: all check-server check-client-package install-client-deps build-client build run run-solo-voice clean-client

all: build

check-server:
	@command -v "$(SERVER_BIN)" >/dev/null || { \
		echo "Missing $(SERVER_BIN). Run 'make install-space-game-server' from the Parlando top-level directory, or set SERVER_BIN=/path/to/parlando-space-game."; \
		exit 1; \
	}

check-client-package:
	@test -f "$(LOCAL_CLIENT_PACKAGE)/package.json" || { \
		echo "Missing local yalc package $(CLIENT_PACKAGE)@$(CLIENT_VERSION). Run 'make install-js-client' from the Parlando top-level directory."; \
		exit 1; \
	}

install-client-deps: check-client-package
	cd "$(SPACE_GAME_DIR)" && npm install --no-save "$(LOCAL_CLIENT_PACKAGE)"

build-client: install-client-deps
	cd "$(SPACE_GAME_DIR)" && npm run build

build: build-client check-server

run: build
	mkdir -p "$(SPACE_GAME_DIR)/.local"
	"$(SERVER_BIN)" --host "$(HOST)" --port "$(PORT)" --config "$(CONFIG)"

run-solo-voice:
	$(MAKE) run CONFIG="$(SPACE_GAME_DIR)/config/experiment.local-back-and-forth-voice.yaml"

clean-client:
	cd "$(SPACE_GAME_DIR)" && rm -rf dist node_modules .yalc yalc.lock package-lock.json
