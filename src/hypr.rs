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
    // simpan posisi cursor — dispatcher focus nge-warp ke tengah window,
    // kita warp balik biar cursor nggak nyelip.
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
    // warp cursor balik ke posisi semula
    if let Some((x, y)) = cursor {
        run(&["hyprctl", "dispatch", &format!("hl.dsp.cursor.move({{ x = {}, y = {} }})", x, y)]);
    }
}

/// Launch command via compositor, to ensure the Hyprland env is correct.
pub fn spawn(command: &str) {
    run(&[
        "hyprctl",
        "dispatch",
        &format!("hl.dsp.exec_cmd({})", lua_str(command)),
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
