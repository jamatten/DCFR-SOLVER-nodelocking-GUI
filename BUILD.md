# Build Guide

This document describes every build command for the DCFR Solver project, what each does, and when to use it.

The project has **two build systems** that work together:

1. **Rust/Cargo** — the solver library (`dcfr-solver`) and CLI binary
2. **Tauri + SvelteKit + Vite** — the desktop GUI application

---

## Table of Contents

- [Prerequisites](#prerequisites)
- [Rust Backend (Solver Library + CLI)](#rust-backend-solver-library--cli)
- [Frontend / GUI (Tauri + SvelteKit)](#frontend--gui-tauri--sveltekit)
- [Quick Reference](#quick-reference)
- [Typical Workflows](#typical-workflows)

---

## Prerequisites

- **Rust 1.70+** — install via [rustup](https://rustup.rs/)
- **Node.js 18+** — install via [nvm](https://github.com/nvm-sh/nvm) or [nodejs.org](https://nodejs.org/)
- **Tauri system dependencies** — see [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)

---

## Rust Backend (Solver Library + CLI)

All commands in this section run from the **project root** (`DCFR-SOLVER-nodelocking-GUI/`).

### `cargo build`

- **What it does:** Compiles the `dcfr-solver` library and CLI binary in **debug mode**.
- **Optimization:** `opt-level = 0` for the solver crate, `opt-level = 3` for dependencies.
- **When to use:** Quick iteration when developing solver logic — just checking it compiles. **Not suitable for actual solving** (too slow).

### `cargo build --release`

- **What it does:** Compiles with full optimizations (`opt-level = 3`, fat LTO, single codegen unit).
- **When to use:** **Primary build command for solver work.** Use this whenever you want to run the solver for real solves. The release binary is 10–100× faster than debug.

### `cargo build --release --features nn`

- **What it does:** Enables the optional `nn` feature, pulling in the `ort` (ONNX Runtime) dependency for neural-network depth-limited solving.
- **When to use:** Only when you need the `datagen` or `compute-equity` subcommands, or `--depth-limit` / `--valuenet` options. Without this flag, those subcommands are hidden.

### `cargo run --release -- <subcommand> [args]`

- **What it does:** Builds (if needed) and immediately runs the release binary with the given CLI subcommand.
- **When to use:** Running a solve without manually locating the binary.

**Examples:**

```bash
# Flop solve
cargo run --release -- solve \
  --board "Td9d6h" \
  --street flop \
  --oop-range "AA-22,AKs-A2s,KQs-K2s,QJs-Q2s,JTs-J2s,T9s-T6s,98s-96s,87s-86s,76s-75s,65s-64s,54s,AKo-A2o,KQo-K9o,QJo-Q9o,JTo-J9o,T9o" \
  --ip-range "AA-22,AKs-A2s,KQs-K2s,QJs-Q2s,JTs-J2s,T9s-T6s,98s-96s,87s-86s,76s-75s,65s-64s,54s,AKo-A2o,KQo-K9o,QJo-Q9o,JTo-J9o,T9o" \
  --pot 30 --stack 170 --iterations 1000 \
  --bet-sizes 67 --raise-sizes 100 --geometric \
  --output result.html

# Turn solve
cargo run --release -- solve \
  --board "AsKs3d7c" --street turn \
  --oop-range "AA,KK,QQ" --ip-range "JJ,TT,99" \
  --pot 2000 --stack 4000 --iterations 2000 \
  --output result.json

# River solve
cargo run --release -- solve \
  --board "AsKs3d7c2h" --street river \
  --oop-range "AA,KK,QQ" --ip-range "JJ,TT,99" \
  --pot 2000 --stack 4000 --iterations 5000 \
  --output result.html

# Preflop solve
cargo run --release -- preflop --iterations 100000000 --output blueprint.bin

# Precompute equity abstraction
cargo run --release -- abstract --output abstraction.bin

# Train blueprint strategy
cargo run --release -- train \
  --abstraction abstraction.bin --output blueprint.bin \
  --iterations 1000000 --stack 100

# Extract preflop chart
cargo run --release -- chart-preflop \
  --blueprint preflop_blueprint.bin --output preflop_charts.json

# Generate batch configs from preflop matchups
cargo run --release -- batch-config \
  --matchups matchups.json --output batch_configs.jsonl

# Run batch solver
cargo run --release -- batch-run \
  --input batch_configs.jsonl --output-dir results
```

### `cargo test --release`

- **What it does:** Runs all unit + integration tests (`tests/integration.rs`, `tests/iso_differential.rs`) in release mode.
- **When to use:** After making changes to verify correctness. Run before committing.

### `cargo test --release -- --ignored`

- **What it does:** Runs long-running validation tests marked `#[ignore]`.
- **When to use:** Full convergence/exploitability validation (takes minutes or longer).

### `cargo clippy`

- **What it does:** Runs the Rust linter for code quality suggestions.
- **When to use:** Optional code quality check.

### `cargo fmt`

- **What it does:** Auto-formats all Rust code.
- **When to use:** Before committing, to keep code style consistent.

---

## Frontend / GUI (Tauri + SvelteKit)

All commands in this section run from the **`gui/`** directory.

### `npm install`

- **What it does:** Installs all Node.js dependencies (SvelteKit, Vite, Tauri CLI, svelte-check, etc.).
- **When to use:** First-time setup, or after pulling changes that modify `gui/package.json`.

### `npm run dev` (aka `vite dev`)

- **What it does:** Starts the Vite dev server on port 1420 with hot-module replacement (HMR). Serves the SvelteKit frontend only — **no Rust backend**.
- **When to use:** Frontend-only changes (UI, layout, styling, Svelte components). Fast refresh, no Rust compilation needed.

### `npm run build` (aka `vite build`)

- **What it does:** Produces a production build of the SvelteKit frontend into `gui/build/` (static SPA mode via `@sveltejs/adapter-static`).
- **When to use:** Building the frontend for production. Also run **automatically** by `tauri build` (configured as `beforeBuildCommand` in `tauri.conf.json`).

### `npm run preview` (aka `vite preview`)

- **What it does:** Serves the production frontend build locally for preview.
- **When to use:** Verifying the production frontend build looks correct before bundling with Tauri.

### `npm run check`

- **What it does:** Runs `svelte-kit sync` + `svelte-check` for TypeScript/Svelte type checking.
- **When to use:** After frontend changes to catch type errors.

### `npm run check:watch`

- **What it does:** Same as `check` but in watch mode (re-runs on file changes).
- **When to use:** During active frontend development for continuous type checking.

### `npm run tauri dev`

- **What it does:** Starts **full-stack development mode**:
  - Runs `npm run dev` (Vite frontend with HMR)
  - Compiles the Tauri Rust backend in **debug mode** (see note below)
  - Launches the desktop app with hot reload
- **When to use:** Primary development command for the GUI. Use when working on anything involving the Tauri IPC bridge, solver commands (`solve_flop`, `solve_turn`, etc.), or testing the full app interactively.

> **⚠️ Performance note:** `npm run tauri dev` compiles the Rust backend in **debug mode** (`opt-level = 0` for the solver crate). The solver will run **significantly slower** than release. This is fine for UI/layout work, but **not suitable for testing actual solver performance or convergence**.
>
> To use the optimized backend during development, pass the `--release` flag:
> ```bash
> npm run tauri dev -- --release
> ```
> This compiles the Rust backend in release mode. First compile takes longer (LTO + single codegen unit), but subsequent incremental compiles are faster, and the solver runs at full speed.

### `npm run tauri build`

- **What it does:** Produces a **distributable desktop app**:
  - Runs `npm run build` (frontend production build)
  - Compiles Tauri Rust backend in **release mode**
  - Bundles into an installer (`.msi`/`.exe` on Windows)
- **Output:** `gui/src-tauri/target/release/bundle/`
- **When to use:** Creating a shippable installer for the GUI app. This is the "production build" of the entire application.

---

## Quick Reference

| Goal | Command | Directory |
|------|---------|----------|
| Build the solver CLI (fast runtime) | `cargo build --release` | Root |
| Run a solve from CLI | `cargo run --release -- solve ...` | Root |
| Build with NN support | `cargo build --release --features nn` | Root |
| Run tests | `cargo test --release` | Root |
| Run long validation tests | `cargo test --release -- --ignored` | Root |
| Format Rust code | `cargo fmt` | Root |
| Install frontend deps | `npm install` | `gui/` |
| Frontend-only dev (no Rust) | `npm run dev` | `gui/` |
| Full app dev (debug Rust backend) | `npm run tauri dev` | `gui/` |
| Full app dev (optimized Rust backend) | `npm run tauri dev -- --release` | `gui/` |
| Type-check frontend | `npm run check` | `gui/` |
| Build frontend for production | `npm run build` | `gui/` |
| Build distributable desktop app | `npm run tauri build` | `gui/` |

---

## Typical Workflows

### Solver development
```bash
cargo build --release                    # compile
cargo run --release -- solve ...         # test a solve
cargo test --release                     # verify correctness
cargo fmt                                # format before commit
```

### GUI development (UI/layout work)
```bash
cd gui
npm install                              # first time only
npm run dev                              # frontend-only, fast HMR
```

### GUI development (testing solver through GUI)
```bash
cd gui
npm run tauri dev -- --release           # full app with optimized Rust
```

### Production GUI release
```bash
cd gui
npm run tauri build                      # produces installer
# Installer at: gui/src-tauri/target/release/bundle/