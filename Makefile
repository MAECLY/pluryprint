# Pluryprint — developer tasks.
#
# This Makefile is a thin wrapper around `pnpm` and `cargo`. It is the
# canonical entry point for local development. Each target documents what
# it does in the help banner. Run `make` (or `make help`) to see them all.
#
# Windows note:
#   GNU make is not bundled with Windows. Install once with one of:
#     winget install GnuWin32.Make           # easiest, no extras
#     scoop install make                     # if you use scoop
#     choco install make                     # if you use chocolatey
#   Or, alternatively, run any of the underlying commands directly
#   (they are listed below each target).

SHELL := /usr/bin/env bash
.DEFAULT_GOAL := help
.ONESHELL:
.SHELLFLAGS := -eu -o pipefail -c
MAKEFLAGS += --no-print-directory --warn-undefined-variables

# ───────────────────────────────────────────────────────────────────────
# Help
# ───────────────────────────────────────────────────────────────────────

.PHONY: help
help: ## Show this help.
	@echo ""
	@echo "  Pluryprint — make targets"
	@echo "  ──────────────────────────"
	@grep -E '^[a-zA-Z][a-zA-Z0-9_-]*:.*## ' $(MAKEFILE_LIST) | \
	  sort | \
	  awk 'BEGIN {FS = ":.*## "}; {printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'
	@echo ""
	@echo "  Quick start:  make install && make dev"
	@echo ""

# ───────────────────────────────────────────────────────────────────────
# Setup
# ───────────────────────────────────────────────────────────────────────

.PHONY: install
install: ## Install all frontend + Rust dependencies.
	pnpm install
	cargo fetch --manifest-path src-tauri/Cargo.toml

.PHONY: doctor
doctor: ## Check that required toolchain pieces are installed.
	@echo "node    : $$(node --version 2>/dev/null || echo 'MISSING')"
	@echo "pnpm    : $$(pnpm --version 2>/dev/null || echo 'MISSING')"
	@echo "rustc   : $$(rustc --version 2>/dev/null || echo 'MISSING')"
	@echo "cargo   : $$(cargo --version 2>/dev/null || echo 'MISSING')"
	@echo "tauri   : $$(pnpm tauri --version 2>/dev/null | tail -n1 || echo 'MISSING')"

# ───────────────────────────────────────────────────────────────────────
# Dev loops
# ───────────────────────────────────────────────────────────────────────

.PHONY: dev
dev: ## Run the full Tauri dev loop (Rust backend + Vite + WebView).
	pnpm tauri dev

.PHONY: dev-web
dev-web: ## Run only Vite (no Rust). Faster iteration on UI-only work.
	pnpm dev

# ───────────────────────────────────────────────────────────────────────
# Builds
# ───────────────────────────────────────────────────────────────────────

.PHONY: build
build: ## Build a release Tauri bundle (unsigned).
	pnpm tauri build

.PHONY: build-web
build-web: ## Build the frontend bundle only (dist/).
	pnpm build

.PHONY: preview
preview: build-web ## Preview the production frontend bundle.
	pnpm preview

# ───────────────────────────────────────────────────────────────────────
# Quality gates
# ───────────────────────────────────────────────────────────────────────

.PHONY: lint
lint: ## Lint TS/TSX with ESLint.
	pnpm lint

.PHONY: lint-fix
lint-fix: ## Auto-fix ESLint issues where possible.
	pnpm lint:fix

.PHONY: format
format: ## Apply Prettier to all source files.
	pnpm format

.PHONY: format-check
format-check: ## Verify Prettier formatting without writing.
	pnpm format:check

.PHONY: typecheck
typecheck: ## Run TypeScript type-checker.
	pnpm typecheck

.PHONY: test
test: ## Run frontend unit tests (vitest).
	pnpm test

.PHONY: test-rust
test-rust: ## Run Rust unit + integration tests.
	cargo test --manifest-path src-tauri/Cargo.toml

.PHONY: test-all
test-all: test test-rust ## Run frontend AND Rust tests.

.PHONY: clippy
clippy: ## Lint Rust with cargo clippy (deny warnings).
	cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings

.PHONY: cargo-fmt
cargo-fmt: ## Format Rust source.
	cargo fmt --manifest-path src-tauri/Cargo.toml --all

.PHONY: cargo-fmt-check
cargo-fmt-check: ## Verify Rust formatting without writing.
	cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check

.PHONY: check
check: lint format-check typecheck test cargo-fmt-check clippy test-rust ## Run every quality gate locally.

.PHONY: audit
audit: ## Audit Rust (cargo audit) and npm (pnpm audit) dependencies.
	cd src-tauri && cargo audit
	cd shell-ext-win && cargo audit
	pnpm audit --prod --audit-level high

.PHONY: check-full
check-full: check audit ## Every gate CI runs, plus the dependency audits audit.yml runs.

# ───────────────────────────────────────────────────────────────────────
# Maintenance
# ───────────────────────────────────────────────────────────────────────

.PHONY: clean
clean: ## Remove node_modules, dist/, and Rust target/.
	@echo "Removing build artifacts and dependency trees…"
	rm -rf node_modules dist src-tauri/target src-tauri/gen .vite

.PHONY: update-deps
update-deps: ## Interactively update frontend deps and run cargo update.
	pnpm update --interactive
	cargo update --manifest-path src-tauri/Cargo.toml

.PHONY: bindings
bindings: ## Regenerate TypeScript bindings via tauri-specta (forces a debug build).
	cargo build --manifest-path src-tauri/Cargo.toml

.PHONY: tauri-icon
tauri-icon: ## Generate Tauri icons from a 1024x1024 source PNG (src-tauri/icons/source.png).
	pnpm tauri icon src-tauri/icons/source.png
