// All communication to Hyprland via `hyprctl`.
// MVP: poll `hyprctl -j clients` + dispatch focus / spawn.

use serde_json::Value;

#[derive(Clone, Debug)]
pub struct Client {
    pub address: String,
    pub class: String,
    pub title: String,
    pub workspace_id: i64,
    pub floating: bool,
    /// Set if the window is in a special workspace.
    pub special_name: Option<String>,
    pub pid: Option<i64>,
    #[allow(dead_code)]
    pub initial_class: Option<String>,
}

fn run(cmd: &[&str]) -> Option<String> {
    String::from_utf8(
        std::process::Command::new(cmd[0])
            .args(&cmd[1..])
            .output()
            .ok()?
            .stdout,
    )
    .ok()
}

/// Compositor cursor position: (x, y) global.
pub fn cursor_pos() -> Option<(i64, i64)> {
    let json = run(&["hyprctl", "-j", "cursorpos"])?;
    let v: Value = serde_json::from_str(&json).ok()?;
    Some((v.get("x")?.as_i64()?, v.get("y")?.as_i64()?))
}

/// Active workspace: (id, name) or None if not available.
pub fn active_workspace() -> Option<(i64, String)> {
    let json = run(&["hyprctl", "-j", "activeworkspace"])?;
    let ws: Value = serde_json::from_str(&json).ok()?;
    Some((
        ws.get("id")?.as_i64()?,
        ws.get("name")?.as_str()?.to_string(),
    ))
}

/// Address of the currently active window, if any.
pub fn active_window_address() -> Option<String> {
    let json = run(&["hyprctl", "-j", "activewindow"])?;
    let v: Value = serde_json::from_str(&json).ok()?;
    Some(v.get("address")?.as_str()?.to_string())
}

/// Close a window by its address.
pub fn close_window(address: &str) {
    run(&[
        "hyprctl",
        "dispatch",
        &format!(
            "hl.dsp.window.close({{ window = {} }})",
            lua_str(&format!("address:{}", address))
        ),
    ]);
}

/// Ensure compositor layer rules (blur, ignore_alpha) are registered for hyprland-dock.
pub fn setup_layer_rules() {
    let _ = run(&[
        "hyprctl",
        "eval",
        "hl.layer_rule({ match = { namespace = 'hyprland-dock' }, blur = true, ignore_alpha = 0.2 })",
    ]);
    let _ = run(&["hyprctl", "keyword", "layerrule", "blur, hyprland-dock"]);
    let _ = run(&[
        "hyprctl",
        "keyword",
        "layerrule",
        "ignorealpha 0.2, hyprland-dock",
    ]);
}

/// List of all mapped client windows.
pub fn clients() -> Vec<Client> {
    let Some(json) = run(&["hyprctl", "-j", "clients"]) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<Value>>(&json)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|c| {
            let mapped = c.get("mapped")?.as_bool()?;
            // hidden = minimized / in non-active workspace; still visible in dock
            if !mapped {
                return None;
            }
            let ws = c.get("workspace").cloned().unwrap_or(Value::Null);
            Some(Client {
                address: c.get("address")?.as_str()?.to_string(),
                class: c.get("class")?.as_str().unwrap_or("").to_string(),
                title: c.get("title")?.as_str().unwrap_or("").to_string(),
                workspace_id: ws.get("id").and_then(|id| id.as_i64()).unwrap_or(-1),
                floating: c.get("floating").and_then(|f| f.as_bool()).unwrap_or(false),
                special_name: ws
                    .get("name")
                    .and_then(|n| n.as_str())
                    .and_then(|n| n.strip_prefix("special:"))
                    .map(|n| n.to_string()),
                pid: c.get("pid").and_then(|p| p.as_i64()),
                initial_class: c.get("initialClass").and_then(|s| s.as_str()).map(|s| s.to_string()),
            })
        })
        .collect()
}

/// Escape string to Lua literal ("...").
fn lua_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Focus window: one call to `hl.dsp.focus` directly moves to the window's
/// workspace. Windows in special workspaces are opened via toggle special.
pub fn focus(client: &Client) {
    // Save cursor position — focus dispatcher warps to center of the window,
    // warp back so cursor doesn't jump.
    let cursor = cursor_pos();

    if let Some(special) = &client.special_name {
        run(&[
            "hyprctl",
            "dispatch",
            &format!("hl.dsp.workspace.toggle_special({})", lua_str(special)),
        ]);
        if let Some((x, y)) = cursor {
            run(&["hyprctl", "dispatch", &format!("hl.dsp.cursor.move({{ x = {}, y = {} }})", x, y)]);
        }
        return;
    }
    run(&[
        "hyprctl",
        "dispatch",
        &format!(
            "hl.dsp.focus({{ window = {} }})",
            lua_str(&format!("address:{}", client.address))
        ),
    ]);
    // Warp cursor back to previous position
    if let Some((x, y)) = cursor {
        run(&["hyprctl", "dispatch", &format!("hl.dsp.cursor.move({{ x = {}, y = {} }})", x, y)]);
    }
}

/// Launch command via compositor, to ensure the Hyprland env is correct.
pub fn spawn(command: &str) {
    let resolved = if !crate::config::binary_exists(command) {
        let (_, rcmd) = crate::config::resolve_app_info(command, None);
        if crate::config::binary_exists(&rcmd) {
            rcmd
        } else {
            command.to_string()
        }
    } else {
        command.to_string()
    };

    run(&[
        "hyprctl",
        "dispatch",
        &format!("hl.dsp.exec_cmd({})", lua_str(&resolved)),
    ]);
}

/// Icon size in pixels. Change this to resize dock icons.
pub const ICON_SIZE: i32 = 36;

/// Icon from window class, fallback to the class itself.
pub fn icon_image(icon_theme: &gtk4::IconTheme, class: &str, icon: &str) -> gtk4::Image {
    let candidates = [icon, class, &class.to_lowercase()];
    let name = candidates
        .iter()
        .find(|n| icon_theme.has_icon(n))
        .map(|n| n.as_ref())
        .unwrap_or("application-x-executable");
    let img = gtk4::Image::from_icon_name(name);
    img.set_pixel_size(ICON_SIZE);
    img
}

/// Collect active and special workspace IDs across all connected monitors.
pub fn active_workspace_ids() -> Vec<i64> {
    let Some(json) = run(&["hyprctl", "-j", "monitors"]) else {
        return Vec::new();
    };
    let Ok(list) = serde_json::from_str::<Vec<Value>>(&json) else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for m in list {
        if let Some(id) = m
            .get("activeWorkspace")
            .and_then(|w| w.get("id"))
            .and_then(|i| i.as_i64())
        {
            ids.push(id);
        }
        if let Some(sid) = m
            .get("specialWorkspace")
            .and_then(|w| w.get("id"))
            .and_then(|i| i.as_i64())
        {
            if sid != 0 {
                ids.push(sid);
            }
        }
    }
    ids
}

static PREVIEW_CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>> =
    std::sync::OnceLock::new();

fn preview_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>> {
    PREVIEW_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Capture a JPEG screenshot of a running window by its address via `grim`.
/// Uses `grim -T <stableId>` to directly capture toplevel surfaces on any workspace,
/// with geometry crop and cache fallbacks.
pub fn capture_window_preview(address: &str) -> Option<Vec<u8>> {
    let json = run(&["hyprctl", "-j", "clients"])?;
    let list: Vec<Value> = serde_json::from_str(&json).ok()?;

    // Prune closed windows from cache
    let active_addresses: std::collections::HashSet<&str> = list
        .iter()
        .filter_map(|c| c.get("address").and_then(|a| a.as_str()))
        .collect();
    if let Ok(mut cache) = preview_cache().lock() {
        cache.retain(|k, _| active_addresses.contains(k.as_str()));
    }

    let active_ws = active_workspace_ids();

    for c in &list {
        if c.get("address")?.as_str()? == address {
            let mapped = c.get("mapped").and_then(|m| m.as_bool()).unwrap_or(false);
            if !mapped {
                return None;
            }

            // Method 1: Target Wayland toplevel buffer via grim -T <stableId> (captures any workspace!)
            if let Some(stable_id) = c.get("stableId").and_then(|s| s.as_str()) {
                if !stable_id.is_empty() {
                    if let Ok(output) = std::process::Command::new("grim")
                        .args(["-T", stable_id, "-t", "jpeg", "-q", "60", "-"])
                        .output()
                    {
                        if output.status.success() && !output.stdout.is_empty() {
                            let bytes = output.stdout;
                            if let Ok(mut cache) = preview_cache().lock() {
                                cache.insert(address.to_string(), bytes.clone());
                            }
                            return Some(bytes);
                        }
                    }
                }
            }

            // Method 2: Geometry crop fallback if window is on active monitor workspace
            let ws_id = c
                .get("workspace")
                .and_then(|w| w.get("id"))
                .and_then(|i| i.as_i64())
                .unwrap_or(-1);
            let pinned = c.get("pinned").and_then(|p| p.as_bool()).unwrap_or(false);
            let is_on_active_screen = pinned || active_ws.contains(&ws_id);

            if is_on_active_screen {
                let at = c.get("at")?.as_array()?;
                let size = c.get("size")?.as_array()?;
                if at.len() >= 2 && size.len() >= 2 {
                    let x = at[0].as_i64()?;
                    let y = at[1].as_i64()?;
                    let w = size[0].as_i64()?;
                    let h = size[1].as_i64()?;
                    if w > 0 && h > 0 {
                        let geom = format!("{},{} {}x{}", x, y, w, h);
                        if let Ok(output) = std::process::Command::new("grim")
                            .args(["-t", "jpeg", "-q", "60", "-g", &geom, "-"])
                            .output()
                        {
                            if output.status.success() && !output.stdout.is_empty() {
                                let bytes = output.stdout;
                                if let Ok(mut cache) = preview_cache().lock() {
                                    cache.insert(address.to_string(), bytes.clone());
                                }
                                return Some(bytes);
                            }
                        }
                    }
                }
            }

            // Method 3: Fallback to cached thumbnail if previously captured
            if let Ok(cache) = preview_cache().lock() {
                if let Some(cached) = cache.get(address) {
                    return Some(cached.clone());
                }
            }

            return None;
        }
    }
    None
}

/// Scale raw image bytes to an exact target width while maintaining aspect ratio,
/// and return a GTK4 Texture with those exact dimensions.
pub fn create_preview_texture(bytes: &[u8], target_width: i32) -> Option<gtk4::gdk::Texture> {
    use gdk_pixbuf::prelude::*;
    let loader = gdk_pixbuf::PixbufLoader::new();
    loader.write(bytes).ok()?;
    loader.close().ok()?;
    let pixbuf = loader.pixbuf()?;
    let orig_w = pixbuf.width();
    let orig_h = pixbuf.height();
    if orig_w <= 0 || orig_h <= 0 {
        return None;
    }
    let target_h = ((orig_h as f64 / orig_w as f64) * target_width as f64).round() as i32;
    let scaled = pixbuf.scale_simple(
        target_width,
        target_h.max(1),
        gdk_pixbuf::InterpType::Bilinear,
    )?;
    Some(gtk4::gdk::Texture::for_pixbuf(&scaled))
}


