#!/bin/sh
set -eu

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) TARGET=aarch64-apple-darwin ;;
  Darwin-x86_64) TARGET=x86_64-apple-darwin ;;
  *) echo "This script builds native macOS binaries. Use build-desktop.ps1 on Windows." >&2; exit 1 ;;
esac

rustup target add "$TARGET"
cargo test
cargo build --release --target "$TARGET"
mkdir -p dist
cp "target/$TARGET/release/quietwrite" "dist/quietwrite-0.4.2-$TARGET"
echo "Built dist/quietwrite-0.4.2-$TARGET"
