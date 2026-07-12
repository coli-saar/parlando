SHELL := /bin/bash

PARLANDO_DIR := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
LOCAL_PREFIX ?= $(PARLANDO_DIR)/.local
LOCAL_BIN := $(LOCAL_PREFIX)/bin

RUST_SERVER_DIR := $(PARLANDO_DIR)/rust-server
JS_CLIENT_DIR := $(PARLANDO_DIR)/js-client
SPACE_GAME_DIR := $(PARLANDO_DIR)/space-game

HOST ?= 127.0.0.1
PORT ?= 8000

.PHONY: all install-local install-space-game-server install-rust-server install-js-client build-space-game run-space-game run-space-game-solo-voice clean-space-game publish-local package-rust-server-local publish-js-client-local publish-dry-run publish-rust-server-dry-run publish-rust-server

all: install-local build-space-game

install-local: install-space-game-server install-js-client

install-space-game-server:
	cd "$(PARLANDO_DIR)" && cargo install --path space-game/server --root "$(LOCAL_PREFIX)" --force

install-rust-server: install-space-game-server

install-js-client:
	cd "$(JS_CLIENT_DIR)" && npm install
	cd "$(JS_CLIENT_DIR)" && npm run yalc

build-space-game: install-local
	cd "$(SPACE_GAME_DIR)" && PATH="$(LOCAL_BIN):$$PATH" make build

run-space-game: install-local
	cd "$(SPACE_GAME_DIR)" && PATH="$(LOCAL_BIN):$$PATH" make run HOST="$(HOST)" PORT="$(PORT)"

run-space-game-solo-voice: install-local
	cd "$(SPACE_GAME_DIR)" && PATH="$(LOCAL_BIN):$$PATH" make run-solo-voice HOST="$(HOST)" PORT="$(PORT)"

clean-space-game:
	cd "$(SPACE_GAME_DIR)" && make clean-client

publish-local: package-rust-server-local publish-js-client-local

package-rust-server-local:
	cd "$(RUST_SERVER_DIR)" && cargo package --allow-dirty

publish-js-client-local:
	cd "$(JS_CLIENT_DIR)" && npm run yalc

publish-dry-run: publish-rust-server-dry-run

publish-rust-server-dry-run:
	cd "$(RUST_SERVER_DIR)" && cargo publish --dry-run --allow-dirty

publish-rust-server:
	cd "$(RUST_SERVER_DIR)" && cargo publish
