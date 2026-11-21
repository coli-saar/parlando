SHELL := /bin/bash

SPACE_GAME_DIR := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
HOST ?= 127.0.0.1
PORT ?= 8000

.PHONY: all build run clean

all: build

# Build the WebAssembly component and browser client into a portable package.
build:
	"$(SPACE_GAME_DIR)/run.sh" --build-only

# Serve the package with the authored Back and Forth agent.
run:
	HOST="$(HOST)" PORT="$(PORT)" "$(SPACE_GAME_DIR)/run.sh"

# Remove output built by the component CLI.
clean:
	rm -rf "$(SPACE_GAME_DIR)/dist" "$(SPACE_GAME_DIR)/client/dist"
