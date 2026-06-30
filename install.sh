#!/bin/bash
set -euo pipefail

echo "=== stl-gallery-rs installer ==="

INSTALL_DIR="${INSTALL_DIR:-$HOME/projects/stl-gallery-rs}"
BIN_NAME="stl-gallery-rs"

if [ -d "$INSTALL_DIR" ]; then
  echo "Project already exists at $INSTALL_DIR. Pulling latest..."
  git -C "$INSTALL_DIR" pull --ff-only || true
else
  echo "Cloning from GitHub..."
  mkdir -p "$(dirname "$INSTALL_DIR")"
  git clone https://github.com/samwarner16/stl-gallery-rs.git "$INSTALL_DIR"
fi

echo "Building release binary..."
cd "$INSTALL_DIR"
cargo build --release

BINARY="$INSTALL_DIR/target/release/$BIN_NAME"
if [ -f "$BINARY" ]; then
  echo "Build successful: $BINARY"
  echo ""
  echo "To use easily, add to PATH or symlink:"
  echo "  mkdir -p ~/.local/bin"
  echo "  ln -sf \"$BINARY\" ~/.local/bin/$BIN_NAME"
  echo ""
  echo "Then run: $BIN_NAME -i yourmodel.stl -o gallery --width 2048 --height 2048"
else
  echo "Build failed."
  exit 1
fi
