# QuietWrite

A tiny distraction-free terminal writer for Linux, macOS, and Windows, with a direct-framebuffer mode for the original Raspberry Pi Zero W.

QuietWrite stays deliberately small: one native binary, a focused shelf browser and editor, plain Markdown, and no Python runtime or desktop environment.

## What it does

- Starts directly in a terminal
- Opens a keyboard-driven shelf and document browser
- Saves Markdown in `~/Writing`
- Organizes new writing into `~/Writing/Notes`, `~/Writing/Poems`, or `~/Writing/Secret Thoughts`
- Keeps existing Markdown files directly in `~/Writing` visible under Notes without moving them
- Autosaves every two seconds using write-sync-rename replacement
- Shows only word and character counts—no NLP, telemetry, or writing analysis
- Adds gentle blank-page prompts and whimsical save confirmations
- Detects the framebuffer resolution and automatically chooses a large font
- Renders directly to 16-bit or 32-bit Linux framebuffers without a desktop
- Opens a complete, smaller-font shortcut guide with `F1`
- Opens the writing browser with `F2`
- Cycles four playful color themes with `F5` and remembers the setting
- Uses Ratatui and Crossterm for a colored, resize-aware SSH interface
- Rotates through all four orientations with `F6` and remembers the setting
- Adjusts text size with `F7` (larger) and `F8` (smaller)
- Uses a large console font as a fallback when framebuffer access is unavailable
- Supports normal typing, paste, Unicode, Home/End, Delete, and Page Up/Down
- Makes Up/Down follow visible soft-wrapped rows, with automatic cursor-follow scrolling
- Creates a note with `Ctrl+N`
- Saves with `Ctrl+S`
- Saves and exits with `Ctrl+Q`
- Stores display preferences in `~/.config/quietwrite/display.conf`
- Accepts USB, 2.4 GHz receiver, and Bluetooth keyboards through Linux

`Secret Thoughts` requires a password inside QuietWrite. On first entry, the app asks you to create and confirm it; later entries require masked password input. This is an app lock, not encryption: the Markdown files retain normal filesystem protections and remain readable outside QuietWrite.

## Install on a Pi Zero W

Use the ARMv6 release bundle. Copy it to the Pi, unpack it, and run:

```sh
cd quietwrite-armv6
./install.sh --autoboot
sudo reboot
```

The installer puts one executable at `/usr/local/bin/quietwrite`, creates `~/Writing`, and optionally installs a systemd service on `tty1`. SSH remains available while QuietWrite runs on the display.

When launched from an SSH session, QuietWrite automatically uses its terminal interface instead of drawing to the Pi framebuffer. `QUIETWRITE_TERMINAL=1 quietwrite` can still force terminal mode in other environments.

## Desktop platforms

QuietWrite's Ratatui/Crossterm interface builds natively on macOS and Windows. The Linux-only framebuffer renderer is excluded automatically on those platforms; writing, shelves, themes, autosave, and the Secret Thoughts app lock remain available.

Default locations:

- Writing: `~/Writing` on macOS/Linux and `%USERPROFILE%\Writing` on Windows
- Settings: `~/Library/Application Support/QuietWrite` on macOS
- Settings: `%APPDATA%\QuietWrite` on Windows
- Settings: `$XDG_CONFIG_HOME/quietwrite` or `~/.config/quietwrite` on Linux

Build on a Mac:

```sh
./scripts/build-desktop.sh
```

Build on Windows from PowerShell with the Rust MSVC toolchain installed:

```powershell
.\scripts\build-desktop.ps1
```

The **Desktop builds** GitHub Actions workflow can also be started manually. It produces Windows x64, macOS Apple Silicon, and macOS Intel archives without changing the Pi release.

If another writing application already owns `tty1`, disable its direct-boot service before rebooting.

To run without auto-boot:

```sh
./install.sh
quietwrite
```

To remove QuietWrite while retaining notes:

```sh
./install.sh --uninstall
```

## Command line

```text
quietwrite [--new] [--dir DIRECTORY] [FILE]
```

Set `QUIETWRITE_DIR` to change the default note directory.

## Build and test

```sh
cargo test
cargo build --release
```

The Pi Zero W uses ARMv6 with hard-float Linux. The release bundle is a static `arm-unknown-linux-musleabihf` binary compiled for `arm1176jzf-s`; an ARMv7 binary will fail with an illegal-instruction error. Always test release artifacts on real ARMv6 hardware.

## Design boundaries

Version 0.4 intentionally omits search, mouse support, dictation, NLP, synchronization, and file encryption. Files stay readable without QuietWrite, and the editor remains responsive on a single-core Pi Zero W.
