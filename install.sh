#!/bin/sh
# Installs the `sitecannon` binary: a prebuilt release when one exists for this platform,
# otherwise a source build with cargo.
#   curl -fsSL https://raw.githubusercontent.com/vallafederico/sitecannon/main/install.sh | sh
set -eu

REPO="vallafederico/sitecannon"
BIN_DIR="${SITECANNON_BIN_DIR:-$HOME/.local/bin}"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target="aarch64-apple-darwin" ;;
  Darwin-x86_64) target="x86_64-apple-darwin" ;;
  Linux-x86_64) target="x86_64-unknown-linux-gnu" ;;
  *) target="" ;;
esac

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

if [ -n "$target" ] && curl -fsSL "https://github.com/$REPO/releases/latest/download/sitecannon-$target.tar.gz" -o "$tmp/sc.tgz" 2>/dev/null; then
  mkdir -p "$BIN_DIR"
  tar -xzf "$tmp/sc.tgz" -C "$tmp"
  mv "$tmp/sitecannon" "$BIN_DIR/sitecannon"
  chmod +x "$BIN_DIR/sitecannon"
  echo "Installed $BIN_DIR/sitecannon"
else
  echo "No prebuilt binary for this platform yet; building from source (a few minutes)."
  if ! command -v cargo >/dev/null 2>&1; then
    echo "cargo not found. Install Rust first: https://rustup.rs" >&2
    exit 1
  fi
  cargo install --locked --git "https://github.com/$REPO" sitecannon
  BIN_DIR="$HOME/.cargo/bin"
fi

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "Add $BIN_DIR to your PATH, e.g.: export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac
"$BIN_DIR/sitecannon" --version
