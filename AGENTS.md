# AGENTS.md

## Project Overview

`ruterm` is a Rust workspace for a desktop terminal emulator. It starts a shell in a pseudo-terminal, parses terminal output, maintains terminal state, and renders the screen in a window with wgpu. The application is under development; see `README.md` for current features, requirements, controls, and configuration.

## Repository Layout

- `Cargo.toml`: Workspace members and shared dependencies. `Cargo.lock` is the workspace lockfile.
- `crates/parser/` (`ruterm-parser`): Incremental parsing of UTF-8 and terminal escape sequences into actions. `src/lib.rs` drives the parser, while `action.rs`, `sequence.rs`, and `utf8.rs` define actions and parsing helpers.
- `crates/core/` (`ruterm-core`): Terminal state and behavior. `src/terminal/` processes parser actions; `src/screen.rs` manages cells and scrollback; `src/attributes.rs` defines text attributes and colors.
- `crates/pty/` (`ruterm-pty`): Unix pseudo-terminal and child process lifecycle, including spawn, resize, I/O, and termination.
- `crates/desktop/` (`ruterm-desktop`): Application entry point, window and input handling, font configuration, clipboard, and rendering in `src/main.rs`; GPU shader in `src/shader.wgsl`.
- `flake.nix`: Nix development shell and package definition.

Keep escape-sequence recognition in `parser`, terminal semantics in `core`, process and PTY operations in `pty`, and window, input, and rendering concerns in `desktop`. When adding a crate, register it in the root workspace manifest and update affected documentation and Nix definitions.

## Development Workflow

Run commands from the repository root. Use `nix develop` when available; otherwise use a recent stable Rust toolchain and the Linux graphical and font dependencies listed in `README.md`.

```sh
cargo fmt --all --check
cargo check --workspace --locked
cargo test --workspace --locked
```

Use `cargo check -p <package>` or `cargo test -p <package>` for focused work, then run the relevant workspace checks. Run the application with:

```sh
cargo run --release -p ruterm-desktop
```

The desktop application needs a graphical Linux session and a monospace font. Unit tests for parser behavior, terminal state, PTY behavior, and desktop helpers live in their respective crates. Add or update tests when behavior changes, especially for escape sequences, screen state, and input handling.

## Development Principles

- Fix the underlying cause rather than adding a temporary workaround.
- Keep changes focused. For a refactoring task, make the changes needed to improve the structure while preserving behavior.
- Do not silently ignore errors; make failures identifiable where possible.
- Prioritize maintainability and runtime performance over ease of implementation.
- Prefer readable code when compatibility work would make the implementation unnecessarily complex.
- Avoid launching external commands for functionality when a library or direct IPC can provide it. Running the configured shell through the PTY is a core feature.
- Ask for clarification when a requirement cannot be determined from the request and repository context.

## Git

- Review the diff and run relevant checks before committing.
- Commit only changes for the current task, with an English Conventional Commit message.
- Do not include unrelated working tree changes.
- Do not push or rewrite existing commits unless explicitly requested.
