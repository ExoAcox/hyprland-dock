# hypr-dock

Minimalist dock for Hyprland — Rust + GTK4 + gtk4-layer-shell.

## Features
- Multi-window support: each open window displays its own icon in the dock
- Pinned applications defined in `~/.config/hypr-dock/apps.conf`
- Running unpinned applications automatically appear at the end of the dock
- Left-click = focus that specific window; if not running = launch app
- Right-click context menu on icons (New Window, Close Window, Pin/Unpin, Lock/Unlock dock)
- Active window indicator (green line + subtle background highlight for currently focused window)
- Drag-and-drop icon reordering with mouse (active when dock is Unlocked)
- Hyprland IPC event socket integration with backup polling

## Build & Install
Easiest method (automatically builds release binary and installs to `~/.local/bin`):
```sh
./install.sh
```

Or using `make`:
```sh
make install
```

Or manually using Cargo:
```sh
cargo build --release
install -Dm755 target/release/hypr-dock ~/.local/bin/hypr-dock
```

## Running
```sh
hypr-dock
```
Or autostart via `hyprland.conf`:
```
exec-once = hypr-dock
```

## Configuration (`dock.conf`)
```toml
icon_size = 36        # px
anchor = "bottom"     # bottom | top
animation_ms = 130    # slide in/out animation duration
icon_theme = "Papirus" # icon theme (dock-only, e.g. "Papirus", "Papirus-Dark", "Papirus-Light")
locked = false        # lock icon positions (prevent drag & drop reorder)
```
Saved at `~/.config/hypr-dock/dock.conf` (automatically generated on first run).

## Applications (`apps.conf`)
```
# class,icon,command
firefox,firefox,firefox
kitty,kitty,kitty
thunar,thunar,thunar
```
- `class` = window class from `hyprctl clients`
- `icon` = icon name from theme (leave blank → fallback to class)
- `command` = command to launch the app

