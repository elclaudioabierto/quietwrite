#!/bin/sh
set -eu

APP=quietwrite
SERVICE=quietwrite.service
STAGE="$HOME/quietwrite-refresh"
NEW="$STAGE/$APP"
INSTALLED="/usr/local/bin/$APP"
NEXT="/usr/local/bin/$APP.next"
PREVIOUS="/usr/local/bin/$APP.previous"

cd "$STAGE"
sha256sum -c quietwrite.sha256
[ -x "$NEW" ] || chmod 0755 "$NEW"

ARCH=$(uname -m)
case "$ARCH" in
  armv6l|armv7l) ;;
  *)
    echo "Refusing to install an ARMv6 build on unexpected architecture: $ARCH" >&2
    exit 1
    ;;
esac

file "$NEW"
"$NEW" --version
systemctl cat "$SERVICE" >/dev/null

echo "Staging the new binary (sudo may prompt)..."
sudo install -m 0755 "$NEW" "$NEXT"
sudo systemctl stop "$SERVICE"

if [ -e "$INSTALLED" ]; then
  sudo cp -p "$INSTALLED" "$PREVIOUS"
fi
sudo mv -f "$NEXT" "$INSTALLED"

if ! sudo systemctl start "$SERVICE" || ! systemctl is-active --quiet "$SERVICE"; then
  echo "The new service did not start; restoring the previous binary." >&2
  if [ -e "$PREVIOUS" ]; then
    sudo cp -p "$PREVIOUS" "$INSTALLED"
    sudo systemctl start "$SERVICE" || true
  fi
  systemctl --no-pager --full status "$SERVICE" || true
  exit 1
fi

echo "Installed version:"
"$INSTALLED" --version
systemctl --no-pager --full status "$SERVICE"
echo "QuietWrite refreshed. Writing data was not modified."
