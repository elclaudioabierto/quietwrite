# QuietWrite

A tiny, private writing desk for Linux, macOS, Windows, and the original Raspberry Pi Zero W.

QuietWrite is one native binary with two visible spaces:

- **Writing** — ordinary local Markdown
- **Secret Thoughts** — password-protected local encryption

No account, cloud service, telemetry, NLP, or publishing platform is required.

## Writing

New documents are saved under `~/Writing/Notes` (or `%USERPROFILE%\Writing\Notes` on Windows).

The Writing view also discovers existing Markdown in place from older QuietWrite layouts:

- Markdown files directly under `~/Writing`
- `Notes`, `Drafts`, `Journal`, `Poems`, and `Ideas`
- Nested Markdown under `Projects`

QuietWrite does not move, rename, rewrite, or merge those files when presenting the unified Writing view. It excludes Secret Thoughts, Archive, Trash, Exports, and `.quietwrite` internal data.

Writing features include:

- autosave using write-sync-rename replacement
- up to 20 local versions per document
- undo and redo
- restored cursor position
- browser search, pinning, and rename
- safe deletion to `~/Writing/.quietwrite/Trash`
- find within the current document
- optional 25-minute writing sprints and word targets
- Markdown heading outline and heading navigation
- a read-only reference split
- plain word and character counts

Trash is intentionally not a third menu space. Deleted ordinary files remain recoverable from `~/Writing/.quietwrite/Trash` with normal filesystem tools.

## Secret Thoughts

Secret Thoughts is an encrypted local vault. On first entry, QuietWrite asks for an eight-or-more-character password twice. It derives a key with Argon2id and encrypts each note and its version history with authenticated XChaCha20-Poly1305.

The in-memory key is erased when you leave Secret Thoughts or quit. Existing plaintext Secret Thoughts are encrypted in place after the first successful unlock, so make a backup before migration. Filenames, sizes, modification times, and cursor-state metadata are not encrypted. Forgotten passwords cannot be recovered.

## Keys

### Everywhere

- `F1` — help
- `F2` — Writing / Secret Thoughts menu
- `F5` — theme
- `F6` — rotate the Pi display
- `F7` / `F8` — larger / smaller Pi framebuffer text
- `Ctrl+Q` — save and quit

### Writing browser

- `Enter` — open
- `Ctrl+N` — new document
- `/` or `Ctrl+F` — search names and contents
- `p` — pin or unpin
- `r` — rename
- `d` — move to recoverable trash
- `v` — open as a read-only reference

### Editor

- `Ctrl+S` — save
- `Ctrl+N` — new document
- `Ctrl+Z` / `Ctrl+Y` — undo / redo
- `Ctrl+R` — restore the latest local version
- `Ctrl+F` / `F3` — find / find next
- `F4` — start or stop a writing sprint
- `Ctrl+G` — set a session word target
- `F9` — toggle the Markdown outline
- `F10` — toggle the reference split
- `Ctrl+Up` / `Ctrl+Down` — previous / next heading

## Install on a Pi Zero W

Use the ARMv6 release bundle:

```sh
cd quietwrite-armv6
./install.sh --autoboot
sudo reboot
```

The installer places the executable at `/usr/local/bin/quietwrite`, creates `~/Writing`, and optionally installs a `tty1` systemd service. SSH remains available. SSH sessions automatically use the Ratatui/Crossterm interface; the attached Pi display uses the Linux framebuffer renderer.

To install without auto-boot:

```sh
./install.sh
quietwrite
```

To remove QuietWrite while retaining writing:

```sh
./install.sh --uninstall
```

## Desktop builds

QuietWrite builds natively for Windows x64, macOS Apple Silicon, and macOS Intel. The Linux-only framebuffer renderer is excluded on desktop platforms.

macOS:

```sh
./scripts/build-desktop.sh
```

Windows PowerShell with the Rust MSVC toolchain:

```powershell
.\scripts\build-desktop.ps1
```

The manually triggered **Desktop builds** GitHub Actions workflow creates downloadable archives.

Settings locations:

- Linux: `$XDG_CONFIG_HOME/quietwrite` or `~/.config/quietwrite`
- macOS: `~/Library/Application Support/QuietWrite`
- Windows: `%APPDATA%\QuietWrite`

## Command line

```text
quietwrite [--new] [--dir DIRECTORY] [FILE]
```

Set `QUIETWRITE_DIR` to change the writing directory.

## Build and test

```sh
cargo test
cargo build --release
```

The Pi Zero W release is a static `arm-unknown-linux-musleabihf` binary compiled for `arm1176jzf-s`. An ARMv7 binary will fail on the original Pi Zero W. Test Pi release artifacts on real ARMv6 hardware.

## Design boundary

QuietWrite 0.9 is a minimal private writing desk, not a manuscript publisher or content-management system. Ordinary writing remains readable Markdown; only Secret Thoughts is encrypted.
