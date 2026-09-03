#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
TARGET=arm-unknown-linux-musleabihf
TOOLCHAIN="$ROOT/.tools/rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin"
CROSS="$ROOT/.tools/cross"

if [ ! -x "$TOOLCHAIN/cargo" ] || [ ! -x "$CROSS/usr/bin/arm-linux-gnueabihf-gcc-15" ]; then
  echo "QuietWrite's local Rust or ARM cross toolchain is missing under .tools." >&2
  exit 1
fi

export PATH="$TOOLCHAIN:$CROSS/usr/bin:$PATH"
export CARGO_HOME="$ROOT/.tools/cargo"
export RUSTUP_HOME="$ROOT/.tools/rustup"
export LD_LIBRARY_PATH="$CROSS/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export CARGO_TARGET_ARM_UNKNOWN_LINUX_MUSLEABIHF_LINKER="$CROSS/usr/bin/arm-linux-gnueabihf-gcc-15"
export CC_arm_unknown_linux_musleabihf="$CROSS/usr/bin/arm-linux-gnueabihf-gcc-15"
export AR_arm_unknown_linux_musleabihf="$CROSS/usr/bin/arm-linux-gnueabihf-ar"
export CFLAGS_arm_unknown_linux_musleabihf="--sysroot=$CROSS -march=armv6 -mfpu=vfp -mfloat-abi=hard"

cd "$ROOT"
cargo test

export RUSTFLAGS="-C target-cpu=arm1176jzf-s -C link-arg=--sysroot=$CROSS -C link-arg=-static"
cargo build --release --target "$TARGET"

ARTIFACT="$ROOT/target/$TARGET/release/quietwrite"
VERSION=$(sed -n '/^\[package\]/,/^\[/s/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)
file "$ARTIFACT"
file "$ARTIFACT" | grep -q 'ELF 32-bit.*ARM.*statically linked'
readelf -A "$ARTIFACT" | grep -q 'Tag_CPU_arch: v6'
strings "$ARTIFACT" | grep -Fq "quietwrite $VERSION"

(
  cd "$(dirname "$ARTIFACT")"
  sha256sum quietwrite > quietwrite.sha256
)

echo "ARMv6 release $VERSION ready: $ARTIFACT"
cat "$(dirname "$ARTIFACT")/quietwrite.sha256"
