#!/usr/bin/env bash
set -e

REPO_URL="https://github.com/ExoAcox/hyprland-dock.git"
PREFIX="${PREFIX:-/usr/local}"
BIN_DIR="$PREFIX/bin"

# If run directly via curl or outside repository, clone to a temporary directory
if [ ! -f "Cargo.toml" ]; then
    TMP_DIR=$(mktemp -d)
    trap 'rm -rf "$TMP_DIR"' EXIT
    echo "==> Downloading hyprland-dock..."
    git clone --depth=1 "$REPO_URL" "$TMP_DIR"
    cd "$TMP_DIR"
fi

echo "==> Building hyprland-dock (release)..."
cargo build --release

echo "==> Installing binary to $BIN_DIR..."
# If target directory requires root privileges (e.g. /usr/local/bin), use sudo
if [ -w "$BIN_DIR" ] || [ "$EUID" -eq 0 ]; then
    mkdir -p "$BIN_DIR"
    install -Dm755 target/release/hyprland-dock "$BIN_DIR/hyprland-dock"
else
    echo "==> Requesting sudo permissions to install to $BIN_DIR..."
    sudo mkdir -p "$BIN_DIR"
    sudo install -Dm755 target/release/hyprland-dock "$BIN_DIR/hyprland-dock"
fi

echo "==> Success! hyprland-dock has been installed to: $BIN_DIR/hyprland-dock"
echo "Run the dock with: hyprland-dock"
