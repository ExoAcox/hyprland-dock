// ~/.config/hyprland-dock/apps.conf
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

fn config_dir() -> PathBuf {
    dirs::config_dir()
        .expect("Failed to resolve $XDG_CONFIG_HOME")
        .join("hyprland-dock")
}

fn config_path() -> PathBuf {
    config_dir().join("apps.conf")
}

pub fn default_content() -> String {
    String::from(
        "# Format: class,icon,command\n\
         firefox,firefox,firefox\n\
         kitty,kitty,kitty\n\
         thunar,thunar,thunar\n",
    )
}

pub fn save_pinned(apps: &[PinnedApp]) -> std::io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut out = String::from("# Format: class,icon,command\n");
    for app in apps {
        out.push_str(&format!("{},{},{}\n", app.class, app.icon, app.command));
    }
    fs::write(path, out)
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
    let mut apps = parse_config(&content);

    // Auto-repair any entries where command does not exist in PATH
    let mut modified = false;
    for app in &mut apps {
        if !binary_exists(&app.command) {
            let (new_icon, new_cmd) = resolve_app_info(&app.class, None);
            if binary_exists(&new_cmd) {
                app.command = new_cmd;
                if (app.icon == app.class || !binary_exists(&app.icon)) && !new_icon.is_empty() {
                    app.icon = new_icon;
                }
                modified = true;
            }
        }
    }
    if modified {
        let _ = save_pinned(&apps);
    }

    apps
}

/// Check if an executable or command exists in PATH or as a direct file path.
pub fn binary_exists(bin: &str) -> bool {
    let first = bin.split_whitespace().next().unwrap_or(bin);
    if first.contains('/') {
        return std::path::Path::new(first).is_file();
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            if dir.join(first).is_file() {
                return true;
            }
        }
    }
    false
}

/// Clean an Exec command line by removing field codes (%u, %U, %f, %F, etc.) and trailing flags.
pub fn clean_exec(raw: &str) -> String {
    let mut parts: Vec<&str> = raw.split_whitespace().collect();
    parts.retain(|p| !p.starts_with('%'));
    if let Some(&last) = parts.last() {
        if last == "--" {
            parts.pop();
        }
    }
    parts.join(" ")
}

fn collect_desktop_files(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_desktop_files(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("desktop") {
                out.push(path);
            }
        }
    }
}

/// Resolve proper application (icon, command) from .desktop files or system processes.
pub fn resolve_app_info(class: &str, pid: Option<i64>) -> (String, String) {
    let tc = class.to_lowercase();
    let last_seg = tc.split('.').last().unwrap_or(&tc);
    let last_seg_clean = if last_seg.ends_with("_app") {
        &last_seg[..last_seg.len() - 4]
    } else {
        last_seg
    };

    let mut search_dirs = Vec::new();
    if let Some(data) = dirs::data_dir() {
        search_dirs.push(data.join("applications"));
    }
    if let Some(home) = dirs::home_dir() {
        search_dirs.push(home.join(".local/share/flatpak/exports/share/applications"));
    }
    if let Ok(xdg_data_dirs) = std::env::var("XDG_DATA_DIRS") {
        for d in std::env::split_paths(&xdg_data_dirs) {
            search_dirs.push(d.join("applications"));
        }
    }
    search_dirs.push(PathBuf::from("/usr/local/share/applications"));
    search_dirs.push(PathBuf::from("/usr/share/applications"));
    search_dirs.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));

    let mut desktop_files = Vec::new();
    for d in search_dirs {
        if d.exists() {
            collect_desktop_files(&d, &mut desktop_files);
        }
    }

    let mut best_score = 0;
    let mut best_icon: Option<String> = None;
    let mut best_exec: Option<String> = None;

    for path in desktop_files {
        let file_stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let mut exec_cmd = None;
        let mut icon = None;
        let mut wm_class = None;
        let mut in_desktop_entry = false;

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed == "[Desktop Entry]" {
                in_desktop_entry = true;
                continue;
            }
            if trimmed.starts_with('[') && trimmed != "[Desktop Entry]" {
                in_desktop_entry = false;
                continue;
            }
            if in_desktop_entry {
                if trimmed.starts_with("Exec=") && exec_cmd.is_none() {
                    exec_cmd = Some(clean_exec(&trimmed[5..]));
                } else if trimmed.starts_with("Icon=") && icon.is_none() {
                    icon = Some(trimmed[5..].trim().to_string());
                } else if trimmed.starts_with("StartupWMClass=") && wm_class.is_none() {
                    wm_class = Some(trimmed[15..].trim().to_lowercase());
                }
            }
        }

        let exec_cmd = match exec_cmd {
            Some(cmd) if !cmd.is_empty() => cmd,
            _ => continue,
        };
        let icon = icon.unwrap_or_else(|| file_stem.clone());

        let mut score = 0;
        if let Some(wmc) = &wm_class {
            if wmc == &tc {
                score = 100;
            } else if wmc == last_seg || wmc == last_seg_clean {
                score = 85;
            }
        }
        if score == 0 {
            if file_stem == tc {
                score = 90;
            } else if file_stem == last_seg || file_stem == last_seg_clean {
                score = 80;
            } else if file_stem.contains(last_seg_clean) {
                score = 60;
            }
        }

        if score > best_score {
            best_score = score;
            best_icon = Some(icon);
            best_exec = Some(exec_cmd);
        }
    }

    if let (Some(icon), Some(exec)) = (best_icon, best_exec) {
        return (icon, exec);
    }

    // Fallback: check PID /proc/<pid>/exe
    if let Some(p) = pid {
        let exe_link = format!("/proc/{}/exe", p);
        if let Ok(target) = fs::read_link(&exe_link) {
            let target_str = target.to_string_lossy().to_string();
            let bin_name = target
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&target_str)
                .to_string();
            let cmd = if binary_exists(&bin_name) {
                bin_name
            } else {
                target_str
            };
            return (class.to_string(), cmd);
        }
    }

    // Fallback: if last_seg exists in PATH
    if binary_exists(last_seg_clean) {
        return (class.to_string(), last_seg_clean.to_string());
    }
    if binary_exists(last_seg) {
        return (class.to_string(), last_seg.to_string());
    }

    (class.to_string(), class.to_lowercase())
}

// ~/.config/hyprland-dock/dock.conf   → dock configuration
// ~/.config/hyprland-dock/style.css  → theme (generated if missing)

use serde::Deserialize;

#[derive(Deserialize, Clone)]
#[serde(default)]
pub struct ConfigFile {
    pub icon_size: i32,
    pub anchor: String,
    pub animation_ms: u64,
    pub icon_theme: String,
    pub locked: bool,
    pub window_preview: bool,
    pub window_preview_width: i32,
    pub window_preview_delay: u64,
}

impl Default for ConfigFile {
    fn default() -> Self {
        ConfigFile {
            icon_size: 36,
            anchor: "bottom".to_string(),
            animation_ms: 130,
            icon_theme: "Papirus".to_string(),
            locked: false,
            window_preview: true,
            window_preview_width: 360,
            window_preview_delay: 200,
        }
    }
}

fn toml_path() -> PathBuf {
    config_dir().join("dock.conf")
}

pub fn default_toml() -> String {
    String::from(
        r##"# ~/.config/hyprland-dock/dock.conf

icon_size = 36        # px
anchor = "bottom"     # bottom | top
animation_ms = 130    # duration of the slide in/out animation
icon_theme = "Papirus" # icon theme (dock-only, e.g. "Papirus", "Papirus-Dark", "Papirus-Light")
locked = false        # lock icon positions (prevent drag & drop reorder)
window_preview = true # show window thumbnail preview on hover
window_preview_width = 360 # preview thumbnail width in pixels
window_preview_delay = 200 # hover delay in milliseconds before showing preview
"##,
    )
}

pub fn save_toml(cfg: &ConfigFile) -> std::io::Result<()> {
    let path = toml_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let content = format!(
        "# ~/.config/hyprland-dock/dock.conf\n\n\
         icon_size = {}        # px\n\
         anchor = \"{}\"     # bottom | top\n\
         animation_ms = {}    # duration of the slide in/out animation\n\
         icon_theme = \"{}\" # icon theme\n\
         locked = {}        # lock icon positions (prevent drag & drop reorder)\n\
         window_preview = {} # show window thumbnail preview on hover\n\
         window_preview_width = {} # preview thumbnail width in pixels\n\
         window_preview_delay = {} # hover delay in milliseconds before showing preview\n",
        cfg.icon_size,
        cfg.anchor,
        cfg.animation_ms,
        cfg.icon_theme,
        cfg.locked,
        cfg.window_preview,
        cfg.window_preview_width,
        cfg.window_preview_delay
    );
    fs::write(path, content)
}

pub fn load_toml() -> ConfigFile {
    let path = toml_path();

    if !path.exists() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
            let _ = fs::write(&path, default_toml());
        }
    }

    match fs::read_to_string(&path) {
        Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
            eprintln!("hyprland-dock: dock.conf invalid, falling back to defaults: {e}");
            ConfigFile::default()
        }),
        Err(_) => ConfigFile::default(),
    }
}

// ---- theme CSS ----

pub const DEFAULT_CSS: &str = r#"/** ~/.config/hyprland-dock/style.css — hyprland-dock theme
 * Feel free to customize: colors, hover highlights, indicators, padding.
 * Selectors:
 *   window.dock-window      — dock window (must be transparent for floating icons)
 *   .dock                   — icon container
 *   .dock-btn               — per-app button
 *   .dock-btn:hover         — highlight when hovered
 *   .dock-btn.running       — running app indicator (bottom bar)
 *   .dock-btn.drop-target   — drop target highlight during drag
 *   popover.dock-tooltip    - icon tooltip hover
 *   popover.dock-popover    — right-click context menu
 *   .dock-tooltip-label     - tooltip label
 *   .dock-tooltip-preview   - thumbnail preview
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
    border-radius: 0;
    border-left: 2px solid transparent;
    border-right: 2px solid transparent;
    border-bottom: 2px solid transparent;
    box-shadow: none;
    padding: 6px;
    margin: 0 2px;
}
.dock button.dock-btn image {
    transition: all 200ms cubic-bezier(0.25, 1, 0.5, 1);
}
.dock button.dock-btn:hover image {
    -gtk-icon-transform: scale(1.5) translateY(-6px);
}
.dock button.dock-btn.running {
    border-bottom: 2px solid #89b4fa;
}
.dock button.dock-btn.active {
    border-bottom: 2px solid #a6e3a1;
}
.dock button.dock-btn.drop-target {
    border-left: 2px dashed #a6adc8;
}
.dock.anchor-top button.dock-btn:hover image {
    -gtk-icon-transform: scale(1.5) translateY(6px);
}
popover.dock-popover {
    margin: 16px 0;
}
popover.dock-popover contents {
    padding: 6px;
    border-radius: 12px;
    background-color: rgba(30, 30, 46, 0.95);
    border: 1px solid rgba(255, 255, 255, 0.15);
}
.dock-menu-btn {
    background: transparent;
    border: none;
    border-radius: 8px;
    padding: 6px 12px;
    color: #cdd6f4;
    font-size: 13px;
}
.dock-menu-btn:hover {
    background: rgba(255, 255, 255, 0.12);
    color: #ffffff;
}
.dock-menu-title {
    font-weight: bold;
    font-size: 12px;
    color: #a6adc8;
    padding: 4px 12px 2px 12px;
}
.dock-menu-sep {
    margin: 4px 0;
    opacity: 0.2;
}
popover.dock-tooltip {
    margin: 16px 0;
}
popover.dock-tooltip contents {
    padding: 4px 10px;
    border-radius: 8px;
    background-color: rgba(24, 24, 37, 0.95);
    border: 1px solid rgba(255, 255, 255, 0.12);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.35);
}
.dock-tooltip-label {
    color: #cdd6f4;
    font-size: 12px;
    font-weight: 500;
    margin: 4px 0;
}
.dock-tooltip-preview {
    border-radius: 6px;
    margin-top: 10px;
}
"#;

pub fn default_css() -> String {
    DEFAULT_CSS.to_string()
}

pub fn css_path() -> PathBuf {
    config_dir().join("style.css")
}

pub fn load_css() -> String {
    let path = css_path();

    if !path.exists() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&path, default_css());
        eprintln!("hyprland-dock: style.css does not exist, generated default template");
    }

    fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("hyprland-dock: failed to read style.css, using default template: {e}");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config() {
        let sample = "\
# comment
firefox,firefox,firefox
kitty,,kitty
";
        let apps = parse_config(sample);
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].class, "firefox");
        assert_eq!(apps[0].icon, "firefox");
        assert_eq!(apps[0].command, "firefox");
        assert_eq!(apps[1].class, "kitty");
        assert_eq!(apps[1].icon, "kitty");
        assert_eq!(apps[1].command, "kitty");
    }

    #[test]
    fn test_toml_locked_default() {
        let toml_str = r#"
icon_size = 40
anchor = "bottom"
"#;
        let cfg: ConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.locked, false);
        assert_eq!(cfg.icon_size, 40);
    }

    #[test]
    fn test_toml_locked_explicit() {
        let toml_str = r#"
icon_size = 40
locked = true
"#;
        let cfg: ConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.locked, true);
    }

    #[test]
    fn test_clean_exec() {
        assert_eq!(clean_exec("zeditor %U"), "zeditor");
        assert_eq!(clean_exec("zapzap %u"), "zapzap");
        assert_eq!(
            clean_exec("mpv --player-operation-mode=pseudo-gui -- %U"),
            "mpv --player-operation-mode=pseudo-gui"
        );
        assert_eq!(clean_exec("pavucontrol"), "pavucontrol");
    }

    #[test]
    fn test_binary_exists() {
        assert!(binary_exists("sh"));
        assert!(!binary_exists("definitely_non_existent_binary_xyz_123"));
    }

    #[test]
    fn test_repair_pinned() {
        let sample = "\
dev.zed.Zed,dev.zed.Zed,dev.zed.zed
org.pulseaudio.pavucontrol,org.pulseaudio.pavucontrol,org.pulseaudio.pavucontrol
com.rtosta.zapzap,com.rtosta.zapzap,com.rtosta.zapzap
org.localsend.localsend_app,org.localsend.localsend_app,org.localsend.localsend_app
";
        let mut apps = parse_config(sample);
        for app in &mut apps {
            if !binary_exists(&app.command) {
                let (new_icon, new_cmd) = resolve_app_info(&app.class, None);
                if binary_exists(&new_cmd) {
                    app.command = new_cmd;
                    if (app.icon == app.class || !binary_exists(&app.icon)) && !new_icon.is_empty()
                    {
                        app.icon = new_icon;
                    }
                }
            }
        }
        assert_eq!(apps[0].command, "zeditor");
        assert_eq!(apps[1].command, "pavucontrol");
        assert_eq!(apps[2].command, "zapzap");
        assert_eq!(apps[3].command, "localsend");
    }
}
