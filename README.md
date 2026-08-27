# QuietWrite

A tiny distraction-free terminal writer for Linux, macOS, and Windows, with a direct-framebuffer mode for the original Raspberry Pi Zero W.

QuietWrite stays deliberately small: one native binary, a focused shelf browser and editor, plain Markdown, and no Python runtime or desktop environment.

## What it does

- Starts directly in a terminal
- Opens a keyboard-driven shelf and document browser
- Saves Markdown in `~/Writing`
- Organizes writing into focused shelves for Notes, Drafts, Journal, Poems, Ideas, Projects, Secret Thoughts, Archive, and recoverable Trash
- Keeps existing Markdown files directly in `~/Writing` visible under Notes without moving them
- Autosaves every two seconds using write-sync-rename replacement
- Keeps up to 20 recoverable local versions of each saved document
- Supports undo and redo with `Ctrl+Z` and `Ctrl+Y`
- Restores the last cursor position when reopening a document
- Renames, moves, archives, pins, and safely trashes documents from the browser
- Searches document names and contents within a shelf
- Finds text inside the current draft with `Ctrl+F` and `F3`
- Runs optional 25-minute writing sprints with `F4`
- Tracks words added during the session and optional word targets
- Discovers nested project folders and chapter files without a project database
- Creates KDP-oriented book projects with title, copyright, numbered chapter, and author pages
- Exports an ordered book with one key to a KDP-compatible HTML manuscript and combined Markdown source
- Builds an outline from ordinary Markdown headings
- Jumps between headings and shows an optional outline pane
- Opens another document beside the active draft as a read-only reference
- Shows local time and background-checked internet status in a persistent information bar
- Shows only word and character counts—no NLP, telemetry, or writing analysis
- Adds gentle blank-page prompts and whimsical save confirmations
- Detects the framebuffer resolution and automatically chooses a large font
- Renders directly to 16-bit or 32-bit Linux framebuffers without a desktop
- Opens a complete, smaller-font shortcut guide with `F1`
- Opens the writing browser with `F2`
- Cycles three high-contrast themes with `F5`: white on black, black on white, and Moon Ink
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

`Secret Thoughts` is an encrypted local vault. On first entry, QuietWrite asks you to enter an 8-or-more-character password twice. It derives a key with Argon2id and encrypts each note and its version history with authenticated XChaCha20-Poly1305. Later entries require the password, and the in-memory key is erased when you leave the shelf or quit.

Existing plaintext Secret Thoughts are encrypted in place after the first successful unlock. Keep a backup before migration. Filenames, file sizes, modification times, and cursor-state metadata are not encrypted; forgotten passwords cannot be recovered. Ordinary shelves remain portable Markdown.

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

## Browser shortcuts

Inside a document shelf:

- `/` or `Ctrl+F` searches titles and contents
- `p` pins or unpins the selected document
- `r` renames it
- `m` moves it to the next shelf, then follows it so the moved note stays visible
- `a` moves it to Archive
- `d` moves it to QuietWrite's recoverable trash
- `v` opens it beside the active draft as a reference

Inside the Projects shelf, `j` creates a book with front matter, a numbered starter chapter, and back matter. `c` creates another chapter beside the selected file. Press `e` on any chapter to export the entire book, ordered by filename, to `~/Writing/Exports/Book Name/manuscript.html` and `manuscript.md` (or under `QUIETWRITE_DIR` when configured).

The HTML manuscript is suitable for KDP's reflowable eBook conversion and no-bleed paperback conversion. Always inspect it in Kindle Previewer or KDP Print Previewer. For precise paperback typography, trim, margins, headers, and pagination, convert the combined Markdown to DOCX/PDF with a typesetting tool before upload.

## Long-form shortcuts

- `F9` toggles the Markdown outline
- `F10` toggles the selected reference document
- `Ctrl+Up` and `Ctrl+Down` jump to the previous or next heading
- `Ctrl+K` and `Ctrl+J` provide alternative heading navigation on desktop terminals

## Design boundaries

Version 0.8 intentionally omits mouse support, dictation, NLP, and hosted synchronization. Ordinary shelves stay readable without QuietWrite, while Secret Thoughts are encrypted locally.
