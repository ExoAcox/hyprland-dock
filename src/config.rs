// ~/.config/hypr-dock/apps.conf
// Format per line:  class,icon,command
// Empty lines and `#` comments are skipped.

use std::fs;
use std::path::PathBuf;

#[derive(Clone)]
pub struct PinnedApp {
    pub class: String,
    pub icon: String,
    pub command: String,
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .expect("Failed to resolve $XDG_CONFIG_HOME")
        .join("hypr-dock")
        .join("apps.conf")
}

pub fn default_content() -> String {
    String::from(
        "# Format: class,icon,command\n\
         firefox,firefox,firefox\n\
         kitty,kitty,kitty\n\
         thunar,thunar,thunar\n",
    )
}

pub fn load_pinned() -> Vec<PinnedApp> {
    let path = config_path();

    if !path.exists() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&path, default_content());
    }

    let content = fs::read_to_string(&path).unwrap_or_default();
    parse_config(&content)
}

// ~/.config/hypr-dock/hypr-dock.toml  → konfigurasi [bar]
// ~/.config/hypr-dock/style.css       → theme (di-generate kalo belum ada)

use serde::Deserialize;

#[derive(Deserialize, Clone)]
#[serde(default)]
pub struct ConfigFile {
    pub icon_size: i32,
    pub anchor: String,
    pub animation_ms: u64,
}

impl Default for ConfigFile {
    fn default() -> Self {
        ConfigFile {
            icon_size: 36,
            anchor: "bottom".to_string(),
            animation_ms: 130,
        }
    }
}

fn toml_path() -> PathBuf {
    dirs::config_dir()
        .expect("Failed to resolve $XDG_CONFIG_HOME")
        .join("hypr-dock")
        .join("dock.conf")
}

/// Nama file lama, untuk migrasi otomatis.
fn legacy_path() -> PathBuf {
    dirs::config_dir()
        .expect("Failed to resolve $XDG_CONFIG_HOME")
        .join("hypr-dock")
        .join("hypr-dock.toml")
}

pub fn default_toml() -> String {
    String::from(
        r##"# ~/.config/hypr-dock/dock.conf

icon_size = 36        # px
anchor = "bottom"     # bottom | top
animation_ms = 130    # duration of the slide in/out animation
"##)
}

pub fn load_toml() -> ConfigFile {
    let path = toml_path();

    if !path.exists() {
        // migrasi: pindahin isi hypr-dock.toml lama (kalo ada) ke dock.conf
        if let Ok(old) = fs::read_to_string(legacy_path()) {
            let _ = fs::write(&path, old);
            let _ = fs::remove_file(legacy_path());
            eprintln!("hypr-dock: hypr-dock.toml dimigrasi ke dock.conf");
        } else if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
            let _ = fs::write(&path, default_toml());
        }
    }

    match fs::read_to_string(&path) {
        Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
            eprintln!("hypr-dock: dock.conf invalid, falling back to defaults: {e}");
            ConfigFile::default()
        }),
        Err(_) => ConfigFile::default(),
    }
}

// ---- theme CSS ----

pub fn default_css() -> String {
    String::from(
        r#"/** ~/.config/hypr-dock/style.css — theme hypr-dock
 * Edit bebas: warna, hover, dot, ukuran padding.
 * Selector:
 *   window.dock-window      — window dock (harus transparan biar icon floating)
 *   .dock                   — kontainer icon
 *   .dock-btn               — tombol per app
 *   .dock-btn:hover         — highlight saat cursor di atas
 *   .dock-btn.running       — app lagi jalan (garis bawah)
 */
window.dock-window {
    background: none;
}
.dock {
    background: none;
    padding: 2px 6px;
}
.dock button.dock-btn {
    background: transparent;
    border: none;
    box-shadow: none;
    padding: 4px;
    margin: 0 2px;
}
.dock button.dock-btn:hover {
    background: rgba(255, 255, 255, 0.18);
    border-radius: 12px;
}
.dock button.dock-btn.running {
    box-shadow: inset 0 -3px 0 -1px #89b4fa;
}
"#)
}

pub fn css_path() -> PathBuf {
    dirs::config_dir()
        .expect("Failed to resolve $XDG_CONFIG_HOME")
        .join("hypr-dock")
        .join("style.css")
}

pub fn load_css() -> String {
    let path = css_path();

    if !path.exists() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&path, default_css());
        eprintln!("hypr-dock: style.css belum ada, di-generate dengan template default");
    }

    fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("hypr-dock: gagal baca style.css, pakai template default: {e}");
        default_css()
    })
}

pub fn parse_config(content: &str) -> Vec<PinnedApp> {
    content
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut parts = l.splitn(3, ',');
            let class = parts.next()?.trim().to_string();
            let icon = parts.next()?.trim().to_string();
            let command = parts.next()?.trim().to_string();
            if class.is_empty() || command.is_empty() {
                return None;
            }
            Some(PinnedApp {
                icon: if icon.is_empty() { class.clone() } else { icon },
                class,
                command,
            })
        })
        .collect()
}
