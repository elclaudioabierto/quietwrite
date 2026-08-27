#!/bin/sh
set -eu

APP=quietwrite
PREFIX=/usr/local/bin
SERVICE=/etc/systemd/system/quietwrite.service
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
MODE=${1:-}

if [ "$(id -u)" -eq 0 ]; then
  TARGET_USER=${SUDO_USER:-root}
else
  TARGET_USER=$(id -un)
fi
TARGET_HOME=$(getent passwd "$TARGET_USER" | cut -d: -f6)
[ -n "$TARGET_HOME" ] || TARGET_HOME=$HOME

if [ "$MODE" = "--uninstall" ]; then
  sudo systemctl disable --now quietwrite.service 2>/dev/null || true
  sudo rm -f "$SERVICE" "$PREFIX/$APP"
  sudo systemctl daemon-reload
  echo "QuietWrite removed. Notes remain in $TARGET_HOME/Writing."
  exit 0
fi

BINARY="$SCRIPT_DIR/quietwrite"
if [ ! -x "$BINARY" ]; then
  echo "Missing executable: $BINARY" >&2
  echo "Use the release bundle for your Raspberry Pi." >&2
  exit 1
fi

mkdir -p "$TARGET_HOME/Writing"
sudo install -m 0755 "$BINARY" "$PREFIX/$APP"
echo "Installed $PREFIX/$APP"

FONT_LINE=
LARGE_FONT=/usr/share/consolefonts/Uni2-TerminusBold32x16.psf.gz
if command -v setfont >/dev/null 2>&1 && [ -r "$LARGE_FONT" ]; then
  FONT_LINE="ExecStartPre=+/usr/bin/setfont $LARGE_FONT"
fi

if [ "$MODE" = "--autoboot" ]; then
  if [ -e "$SERVICE" ] && ! grep -q '^# Managed by QuietWrite installer$' "$SERVICE"; then
    echo "Refusing to replace an unmanaged $SERVICE" >&2
    exit 1
  fi
  TEMP=$(mktemp)
  trap 'rm -f "$TEMP"' EXIT HUP INT TERM
  cat >"$TEMP" <<EOF
# Managed by QuietWrite installer
[Unit]
Description=QuietWrite distraction-free editor
After=local-fs.target systemd-user-sessions.service
Conflicts=getty@tty1.service

[Service]
Type=simple
User=$TARGET_USER
Environment=HOME=$TARGET_HOME
Environment=TERM=linux
Environment=LANG=C.UTF-8
WorkingDirectory=$TARGET_HOME
$FONT_LINE
ExecStart=$PREFIX/$APP
Restart=on-failure
RestartSec=1
StandardInput=tty-force
StandardOutput=tty
StandardError=journal
TTYPath=/dev/tty1
TTYReset=yes
TTYVHangup=yes
TTYVTDisallocate=yes

[Install]
WantedBy=multi-user.target
EOF
  sudo install -m 0644 "$TEMP" "$SERVICE"
  sudo systemctl daemon-reload
  sudo systemctl enable quietwrite.service
  echo "Auto-boot enabled. Reboot when ready: sudo reboot"
else
  echo "Run it now: quietwrite"
  echo "Enable direct boot later: ./install.sh --autoboot"
fi
