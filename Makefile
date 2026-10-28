# Paperclip Desktop — build and release entry points.
#
# The package.json scripts remain the source of truth for *what* each step runs;
# this file adds the dependency graph between them. That is the whole point:
# `pnpm dist:mac` re-stages the server every time, which means two full
# `npm install` runs of a ~850 MB dependency tree even when nothing changed.
# Here the staged bundle is a real file target, so it is rebuilt only when the
# inputs actually move.
#
# Run `make` for the target list.

SHELL := /bin/bash
.SHELLFLAGS := -eu -o pipefail -c
.DEFAULT_GOAL := help

# --- Configuration ----------------------------------------------------------

# Architecture to build for: arm64 (Apple Silicon) or x64 (Intel).
ARCH ?= arm64

ifeq ($(ARCH),arm64)
  TRIPLE := aarch64-apple-darwin
else ifeq ($(ARCH),x64)
  TRIPLE := x86_64-apple-darwin
else
  $(error ARCH must be arm64 or x64, got "$(ARCH)")
endif

VERSION := $(shell node -p "require('./package.json').version")

VARIANT      := mac-$(ARCH)
BUNDLE       := build/server-bundle/$(VARIANT)
SERVER_STAMP := $(BUNDLE)/server/package.json
UI_STAMP     := $(BUNDLE)/server/ui-dist/index.html
ADAPTER_STAMP:= $(BUNDLE)/server/paperclip-bundled-adapters.json
BUNDLE_OUT   := src-tauri/target/$(TRIPLE)/release/bundle
RELEASE_DIR  := release/updater

ADAPTER_SRC := $(shell find adapters -type f \( -name '*.ts' -o -name 'package.json' \) -not -path '*/node_modules/*' -not -path '*/dist/*' 2>/dev/null)
RUST_SRC    := $(shell find src-tauri/src -name '*.rs' 2>/dev/null) src-tauri/Cargo.toml
UI_SRC      := $(shell find ui -type f 2>/dev/null)

# Signing. Unset APPLE_CODESIGN_IDENTITY and the nested-signing step refuses to
# run unless ALLOW_UNSIGNED_MACOS_BUILD is explicitly set — see
# scripts/sign-nested-binaries.mjs.
export ALLOW_UNSIGNED_MACOS_BUILD ?= false

.PHONY: help install dev outdated outdated-json server-version test-scripts acu-audit \
        test test-rust test-adapters check fmt lint typecheck verify \
        server ui adapters app dmg dist release \
        doctor clean clean-bundle clean-rust clean-volumes distclean

# --- Help -------------------------------------------------------------------

help: ## Show this help
	@echo "Paperclip Desktop v$(VERSION) — ARCH=$(ARCH) ($(TRIPLE))"
	@echo
	@grep -hE '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[1m%-16s\033[0m %s\n", $$1, $$2}'
	@echo
	@echo "  Build for Intel with:  make dist ARCH=x64"
	@echo "  Unsigned local build:  make dmg ALLOW_UNSIGNED_MACOS_BUILD=true"

# --- Setup ------------------------------------------------------------------

install: ## Install Node tooling from the lockfile
	pnpm install --frozen-lockfile

doctor: ## Check that the toolchain this project needs is present
	@echo "Toolchain:"
	@for tool in node pnpm cargo rustc; do \
		if command -v $$tool >/dev/null; then \
			printf "  %-22s %s\n" "$$tool" "$$($$tool --version 2>/dev/null | tail -1)"; \
		else printf "  %-22s MISSING\n" "$$tool"; fi; \
	done
	@if cargo tauri --version >/dev/null 2>&1; then \
		printf "  %-22s %s\n" "cargo-tauri" "$$(cargo tauri --version 2>&1 | head -1)"; \
	else printf "  %-22s MISSING (cargo install tauri-cli --version '^2')\n" "cargo-tauri"; fi
	@printf "  %-22s %s\n" "rust target" \
		"$$(rustup target list --installed 2>/dev/null | grep -c '$(TRIPLE)' | sed 's/^0$$/MISSING: rustup target add $(TRIPLE)/;s/^1$$/$(TRIPLE)/')"
	@echo "Adapters:"
	@if command -v devin >/dev/null; then \
		printf "  %-22s %s\n" "devin (devin_local)" "$$(devin version 2>&1 | head -1)"; \
	else printf "  %-22s not on PATH — the devin_local adapter needs it at runtime\n" "devin"; fi

# --- Development ------------------------------------------------------------

dev: ## Run the app against the server in node_modules
	pnpm dev

# --- Quality gates ----------------------------------------------------------

test: test-rust test-adapters test-scripts ## Run every test suite

test-rust: ## Run the Rust shell tests
	cd src-tauri && cargo test

test-adapters: ## Run the in-repo adapter tests
	pnpm test:adapters

test-scripts: ## Run the packaging script tests
	pnpm test:scripts

fmt: ## Format Rust sources
	cd src-tauri && cargo fmt

lint: ## Clippy with warnings denied
	cd src-tauri && cargo clippy --all-targets -- -D warnings

typecheck: ## Typecheck the in-repo adapters
	pnpm --dir adapters/paperclip-adapter-devin typecheck

check: ## Formatting + lint + typecheck, no tests
	cd src-tauri && cargo fmt --check
	$(MAKE) lint typecheck

verify: check test ## Everything CI would gate a release on

# --- Packaging --------------------------------------------------------------
#
# Each staging step is a file target so a rebuild only pays for what changed.
# `make -B` forces a full re-stage when you need one.

server: $(SERVER_STAMP) ## Stage @paperclipai/server and the Node runtime
$(SERVER_STAMP): scripts/prepare-server.mjs package.json
	pnpm prepare-server

ui: $(UI_STAMP) ## Stage the Paperclip UI into the server bundle
$(UI_STAMP): scripts/build-ui.mjs $(SERVER_STAMP)
	pnpm build-ui

adapters: $(ADAPTER_STAMP) ## Build and stage the in-repo adapters
$(ADAPTER_STAMP): scripts/bundle-adapters.mjs $(ADAPTER_SRC) $(SERVER_STAMP)
	pnpm bundle-adapters -- --arch $(ARCH)

app: server ui adapters ## Build the .app and .dmg for the selected ARCH
	@$(MAKE) --no-print-directory clean-volumes
	cargo tauri build --target $(TRIPLE) --config src-tauri/tauri.macos-$(ARCH).conf.json

dmg: app ## Alias for `app`; both produce the .app and the .dmg
	@ls -lh "$(BUNDLE_OUT)/dmg/"*.dmg

dist: ## Full pipeline: stage, bundle, collect artifacts
	$(MAKE) app ARCH=$(ARCH)
	$(MAKE) release ARCH=$(ARCH)

release: ## Collect artifacts and write the updater manifest
	pnpm release:manifest -- \
		--version $(VERSION) \
		--bundle-dir src-tauri/target/$(TRIPLE)/release/bundle \
		--output-dir $(RELEASE_DIR)
	@echo
	@ls -lh $(RELEASE_DIR)

# --- Paperclip server version ------------------------------------------------

server-version: ## Show which Paperclip server this build ships
	@printf "  %-24s %s\n" "pinned (package.json)" \
		"$$(node -p "require('./package.json').devDependencies['@paperclipai/server']")"
	@if [ -f "$(BUNDLE)/server/package.json" ]; then \
		printf "  %-24s %s\n" "staged ($(VARIANT))" "$$(node -p "require('./$(BUNDLE)/server/package.json').version")"; \
	else printf "  %-24s not staged yet\n" "staged ($(VARIANT))"; fi
	@app="$$HOME/Applications/Paperclip Desktop.app/Contents/Resources/app-server/server/package.json"; \
	if [ -f "$$app" ]; then \
		printf "  %-24s %s\n" "installed app" "$$(node -p "require('$$app').version")"; \
	else printf "  %-24s not installed in ~/Applications\n" "installed app"; fi

outdated: server-version ## Compare the pinned server against the latest on npm
	@# The server is pinned, never floating: a desktop build must ship the exact
	@# server it was tested against. This target exists because that means nothing
	@# tells you when upstream moves on — you have to ask.
	@latest="$$(npm view @paperclipai/server version 2>/dev/null || true)"; \
	pinned="$$(node -p "require('./package.json').devDependencies['@paperclipai/server']")"; \
	if [ -z "$$latest" ]; then \
		printf "  %-24s unreachable (offline?)\n" "latest on npm"; \
	else \
		printf "  %-24s %s\n" "latest on npm" "$$latest"; \
		if [ "$$latest" = "$$pinned" ]; then echo; echo "  Up to date."; \
		else echo; echo "  Behind. To upgrade:"; \
		     echo "    pnpm add -D @paperclipai/server@$$latest"; \
		     echo "    make clean-bundle && make verify && make dist"; fi; \
	fi

acu-audit: ## Reconcile the ACU ledger against Devin's own totals
	@# Tests prove the arithmetic; only this proves every run reached the ledger.
	pnpm acu:audit

outdated-json: ## Machine-readable form of `outdated`, for CI
	@node -e '\
	  const { execSync } = require("node:child_process"); \
	  const pinned = require("./package.json").devDependencies["@paperclipai/server"]; \
	  let latest = null; \
	  try { latest = execSync("npm view @paperclipai/server version", { encoding: "utf8", stdio: ["ignore","pipe","ignore"] }).trim(); } catch {} \
	  process.stdout.write(JSON.stringify({ pinned, latest, behind: Boolean(latest) && latest !== pinned }) + "\n"); \
	'

# --- Cleaning ---------------------------------------------------------------

clean-volumes: ## Detach leftover DMG volumes that make bundling fail
	@# bundle_dmg.sh dies with "hdiutil: couldn't unmount - Resource busy" when a
	@# previous run left its volume mounted, and the message never names the cause.
	@for volume in /Volumes/dmg.*; do \
		if [ -d "$$volume" ]; then echo "  detaching $$volume"; hdiutil detach "$$volume" -force >/dev/null 2>&1 || true; fi; \
	done
	@rm -f "$(BUNDLE_OUT)/macos/"rw.*.dmg 2>/dev/null || true

clean-bundle: ## Remove the staged server bundle (forces a full re-stage)
	rm -rf build/server-bundle build/server-staging

clean-rust: ## Remove Rust build output
	cd src-tauri && cargo clean

clean: clean-volumes ## Remove packaged artifacts, keeping the staged server
	rm -rf $(RELEASE_DIR) src-tauri/target/$(TRIPLE)/release/bundle
	rm -rf adapters/*/dist

distclean: clean clean-bundle clean-rust ## Remove everything generated
	@echo 'Removed build output. Run: make install && make dist'
