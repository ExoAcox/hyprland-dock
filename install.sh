#!/usr/bin/env bash
set -e

# Target installation directory (default: ~/.local/bin)
PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"

echo "==> Building hypr-dock (release)..."
cargo build --release

echo "==> Installing binary to $BIN_DIR..."
mkdir -p "$BIN_DIR"
install -Dm755 target/release/hypr-dock "$BIN_DIR/hypr-dock"

echo "==> Success! hypr-dock has been installed to: $BIN_DIR/hypr-dock"

# Check if target directory is in PATH
if [[ ":$PATH:" != *":$BIN_DIR:"* ]]; then
    echo ""
    echo "Note: $BIN_DIR is not currently in your \$PATH."
    echo "Add the following line to your ~/.bashrc or ~/.zshrc:"
    echo "    export PATH=\"\$HOME/.local/bin:\$PATH\""
    echo ""
fi

echo "Run the dock with: hypr-dock"

