# hypr-dock

Dock minimal untuk Hyprland — Rust + GTK4 + gtk4-layer-shell.

## Fitur (v0.1 / MVP)
- Pinned apps dari `~/.config/hypr-dock/apps.conf`
- App jalan tapi nggak pinned otomatis nungul di belakang
- Klik = focus window; kalo belum jalan = spawn
- Dot biru bawah icon = lagi jalan
- Poll `hyprctl -j clients` tiap 1.5s

## Build & install
```sh
cargo build --release
install -Dm755 target/release/hypr-dock ~/.local/bin/hypr-dock
```

## Jalankan
```sh
hypr-dock
```
Atau autostart di `hyprland.conf`:
```
exec-once = hypr-dock
```

## Format `hypr-dock.toml`
```toml
[bar]
icon_size = 36        # px
anchor = "bottom"     # bottom | top
animation_ms = 130    # durasi slide masuk/keluar

[theme]
hover = "rgba(255, 255, 255, 0.18)"   # warna highlight icon pas hover
running_dot = "#89b4fa"               # warna garis bawah app jalan
```
File ini di-generate otomatis (dengan nilai default) saat pertama jalan.

## Format `apps.conf`
```
# class,icon,command
firefox,firefox,firefox
kitty,kitty,kitty
thunar,thunar,thunar
```
- `class` = window class di `hyprctl clients`
- `icon` = nama icon theme (kosongin → pakai class)
- `command` = perintah spawn
