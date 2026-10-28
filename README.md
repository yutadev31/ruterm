# ruterm

ruterm is a desktop terminal emulator written in Rust. It runs your shell in a pseudo-terminal, parses terminal escape sequences, and renders the screen in a window using wgpu.

The project is under development. It currently provides a single terminal window with scrollback, text selection, clipboard support, mouse reporting, and input method support.

## Build and run

You need a recent stable Rust toolchain, a graphical Linux session with OpenGL support, fontconfig, and a monospace font. On Ubuntu, install the build dependencies with:

```sh
sudo apt-get install libwayland-dev libxkbcommon-dev libvulkan-dev libfontconfig1-dev
```

Then run the desktop application from the repository root:

```sh
cargo run --release -p ruterm-desktop
```

ruterm starts the shell named by `SHELL`, or `/bin/sh` if `SHELL` is unset. It sets `TERM=xterm-256color` for the shell.

If you use Nix, the repository also provides a development shell and a package:

```sh
nix develop
cargo run --release -p ruterm-desktop
```

```sh
nix build
./result/bin/ruterm-desktop
```

## Configuration

Font settings are read from `~/.config/ruterm/config.toml` at startup:

```toml
[font]
name = "Hack Nerd Font"
size = 12
```

Both fields are optional. The default font name is `monospace`, and the default size is `12`. The size must be between `1` and `128`. If the file cannot be read or contains invalid settings, ruterm uses the defaults and prints an error to standard error.

## Controls

| Action | Input |
| --- | --- |
| Select text | Drag with the left mouse button |
| Copy selection | `Ctrl+Shift+C` |
| Paste from clipboard | `Ctrl+Shift+V` |
| Scroll through history | Mouse wheel |

When an application enables mouse reporting, hold `Shift` to select text or scroll through history instead of sending mouse input to that application.

## Development

The workspace has four crates: `ruterm-parser` parses terminal input, `ruterm-core` maintains terminal state, `ruterm-pty` manages the shell process, and `ruterm-desktop` provides the window and renderer.

Run the workspace checks with:

```sh
cargo fmt --all --check
cargo check --workspace --locked
cargo test --workspace --locked
```

## License

Licensed under the MIT License. See [LICENSE.txt](LICENSE.txt).
