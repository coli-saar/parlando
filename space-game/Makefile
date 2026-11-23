SHELL := /bin/bash

SPACE_GAME_DIR := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
HOST ?= default

.PHONY: all build deploy clean

all: build

# Build the game and Back and forth agent archives.
build:
	"$(SPACE_GAME_DIR)/build.sh"

# Upload the most recent build to the chosen Parlando server.
deploy:
	"$(SPACE_GAME_DIR)/deploy.sh" --host "$(HOST)"

# Remove output built by the component CLI.
clean:
	rm -rf "$(SPACE_GAME_DIR)/dist" "$(SPACE_GAME_DIR)/client/dist"
