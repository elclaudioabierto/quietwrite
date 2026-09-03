# QuietWrite

A tiny, private writing desk for Linux, macOS, Windows, and the original Raspberry Pi Zero W.

QuietWrite is one native binary with two visible spaces:

- **Writing** — ordinary local Markdown
- **Secret Thoughts** — password-protected local encryption

No account, cloud service, telemetry, NLP, or publishing platform is required. Your writing stays in files you control. The status bar always shows the local time, running QuietWrite version, and Wi-Fi connection state so a deployed device can be identified at a glance.

## Project roadmap

QuietWrite is meant to run on useful hardware people already have, rather than require one official device. I intend to publish:

- 3D-printable case files for several QuietWrite builds
- multiple bills of materials (BOMs), covering different displays, keyboards, power options, and readily available computers
- prebuilt binaries and ready-to-use images for as many operating systems as practical
- support for older 32-bit systems where the operating system, Rust toolchain, and hardware make it possible

These resources will be added progressively. Each release will state which platforms and hardware combinations have been built and tested; planned targets should not be treated as supported until they appear in a release.

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

## Bluetooth keyboards

On Linux devices with a Bluetooth adapter, press `F12` to open keyboard setup. Put the keyboard in pairing mode, wait for it to appear, select it with the arrow keys, and press `Enter`. If QuietWrite shows a passkey, type that number on the Bluetooth keyboard and press `Enter` on the keyboard. QuietWrite then marks the keyboard trusted and attempts to connect it automatically.

The pairing screen lists only devices BlueZ identifies as keyboards or Bluetooth HID devices. Pairing is never automatic: QuietWrite acts only on the device you select. Press `r` to scan again and `F12`, `F2`, or `Esc` to leave setup.

This feature uses the operating system's BlueZ service and `bluetoothctl`; QuietWrite does not include its own Bluetooth stack. On Debian or Raspberry Pi OS, install and start BlueZ if needed:

```sh
sudo apt install bluez
sudo systemctl enable --now bluetooth
```

If setup reports a permission or adapter error, verify that Bluetooth is enabled and that the current user is allowed to control BlueZ. The feature is unavailable on systems without `bluetoothctl`.

## Read notes over Wi-Fi

Press `Ctrl+E` to open the Wi-Fi note library screen. QuietWrite starts a temporary read-only web server and shows both a local address such as `http://zen.local:8787` and an access code. On another phone, tablet, or computer connected to the same network:

1. open the displayed address in a browser
2. enter the access code shown by QuietWrite
3. read or download individual Markdown files

Only ordinary Writing notes are exposed. Secret Thoughts, Trash, local version history, cursor state, and other internal files are excluded. Symlinks that resolve outside the writing directory are also excluded. The browser cannot upload, rename, edit, or delete files.

The server is off by default and is not preserved across application restarts. On the Wi-Fi library screen, press `Enter` to start or stop it; press `Ctrl+E`, `F2`, or `Esc` to return to writing. The access code and browser session key are newly generated each time sharing starts.

This is plain HTTP intended for a trusted local network. The access code prevents casual browsing but does not encrypt traffic, and the server listens on the device's available network interfaces. Do not enable it on an untrusted or public network. If the `.local` name does not resolve, use the numeric address displayed by QuietWrite; installing `avahi-daemon` enables `.local` names on common Linux images.

## Git backup

Press `F11` to enable or disable Git backup. QuietWrite detects the system `git` executable and, when enabled:

1. creates a Git repository directly inside the writing directory if one does not already exist there
2. saves an immediate snapshot, then snapshots on explicit save, quit, and at most once every five minutes while QuietWrite is running
3. pushes without force when that repository already has an `origin` remote

QuietWrite never creates or asks for remote credentials. To protect against device loss, create an empty private repository with your preferred Git host, then configure it from a normal terminal:

```sh
cd ~/Writing
git remote add origin YOUR_PRIVATE_REPOSITORY_URL
git push -u origin HEAD
```

Without `origin`, snapshots exist only on the same device. They provide local history but **do not protect against loss of that device**.

This is intentionally backup-first, not bidirectional sync. QuietWrite does not pull, merge, reset, resolve conflicts, or force-push. If another device changes the remote history, the local snapshot remains intact and the push stops with an error; reconcile the repositories using ordinary Git tools before trying again.

Ordinary notes are committed as readable Markdown, so anyone with access to the remote can read them. Use a private repository. Secret Thoughts are committed only in their encrypted on-disk form, together with the password-verification metadata needed for recovery; this metadata can still help an attacker test password guesses, so use a strong unique password. QuietWrite excludes cursor state, local versions, and recoverable Trash from Git backup.

To recover on a replacement device, clone the backup into the desired writing directory and start QuietWrite with `--dir` if needed. The backed-up Secret Thoughts lock is restored automatically when the platform config copy is absent.

## Keys

### Everywhere

- `F1` — help
- `F2` — Writing / Secret Thoughts menu
- `F5` — theme
- `F6` — rotate the Pi display
- `F7` / `F8` — larger / smaller Pi framebuffer text
- `F11` — toggle Git backup
- `F12` — pair or connect a Bluetooth keyboard
- `Ctrl+E` — open the read-only Wi-Fi note library
- `Ctrl+Q` — save, back up when enabled, and quit

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

To rebuild the current source in WSL and safely refresh an existing `zen@zen.local` installation from Windows, run:

```bat
refresh_quietwrite.bat
```

The refresh script runs the host tests, creates and inspects a static ARMv6 release, verifies its checksum on the Pi, keeps the previous installed binary for rollback, restarts the existing service, and confirms its version and active state. It does not replace the service definition or modify `~/Writing`.

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

The manually triggered **Desktop builds** GitHub Actions workflow creates downloadable archives. Broader release coverage—including additional Linux targets and older 32-bit systems where feasible—is planned; see [Project roadmap](#project-roadmap).

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

Every deployable Pi update increments the package patch version so the on-screen status bar and `quietwrite --version` can confirm that the new binary is installed.

The Pi Zero W release is a static `arm-unknown-linux-musleabihf` binary compiled for `arm1176jzf-s`. An ARMv7 binary will fail on the original Pi Zero W. Test Pi release artifacts on real ARMv6 hardware.

## Design boundary

QuietWrite 0.10 is a minimal private writing desk, not a manuscript publisher or content-management system. Ordinary writing remains readable Markdown; only Secret Thoughts is encrypted.

## License

QuietWrite is released under the [MIT License](LICENSE).
