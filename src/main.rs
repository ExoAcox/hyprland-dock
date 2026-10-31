mod config;
mod hypr;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4_layer_shell::{Edge, Layer, LayerShell};

use crate::config::PinnedApp;

/// CSS theme dari file eksternal: ~/.config/hypr-dock/style.css
/// Priority USER biar menang melawan theme GTK.

fn main() {
    let app = gtk4::Application::builder()
        .application_id("io.github.yuju.hyprdock")
        .build();

    app.connect_activate(build_ui);
    std::process::exit(app.run().into());
}

fn build_ui(app: &gtk4::Application) {
    let cfg = std::rc::Rc::new(config::load_toml());

    // CSS provider global — isi dari style.css
    let css = gtk4::CssProvider::new();
    css.load_from_string(&config::load_css());
    gtk4::style_context_add_provider_for_display(
        &gtk4::gdk::Display::default().expect("no display"),
        &css,
        gtk4::STYLE_PROVIDER_PRIORITY_USER,
    );

    let icon_theme = gtk4::IconTheme::for_display(&gtk4::gdk::Display::default().unwrap());

    let win = gtk4::ApplicationWindow::builder()
        .application(app)
        .resizable(false)
        .decorated(false)
        .default_width(100)
        .build();
    win.add_css_class("dock-window");

    // ---- layer shell setup ----
    if !gtk4_layer_shell::is_supported() {
        eprintln!("hypr-dock: compositor nggak support layer-shell");
        std::process::exit(1);
    }
    let anchor_edge = match cfg.anchor.as_str() {
        "top" => Edge::Top,
        _ => Edge::Bottom,
    };
    win.init_layer_shell();
    win.set_layer(Layer::Top);
    win.set_namespace(Some("hypr-dock"));
    win.set_anchor(anchor_edge, true);
    win.set_exclusive_zone(-1); // -1 = ikut tinggi konten, window nggak tabrak

    // ---- konten dock ----
    let outer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    outer.set_halign(gtk4::Align::Center);
    outer.set_hexpand(true);

    let dock = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    dock.set_widget_name("dock");
    dock.add_css_class("dock");
    outer.append(&dock);
    win.set_child(Some(&outer));

    refresh_dock(&dock, &icon_theme, &win, &cfg);

    // poll cadangan 1s (event IPC yang bikin instan)
    let dock_ref = dock.clone();
    let theme_ref = icon_theme.clone();
    let win_ref = win.clone();
    let cfg_ref = cfg.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(1000), move || {
        refresh_dock(&dock_ref, &theme_ref, &win_ref, &cfg_ref);
        glib::ControlFlow::Continue
    });

    // instan: event thread nandain PENDING, source ini (25ms) nge-refresh.
    let dock_fast = dock.clone();
    let theme_fast = icon_theme.clone();
    let win_fast = win.clone();
    let cfg_fast = cfg.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(25), move || {
        if EVENT_PENDING.swap(false, std::sync::atomic::Ordering::Relaxed) {
            refresh_dock(&dock_fast, &theme_fast, &win_fast, &cfg_fast);
        }
        glib::ControlFlow::Continue
    });
    start_event_listener();

    // Hotzone: cursor nyampah area dock pas dock sembunyi -> dock muncul.
    let dock_hot = dock.clone();
    let theme_hot = icon_theme.clone();
    let win_hot = win.clone();
    let cfg_hot = cfg.clone();
    let was_hot = std::cell::Cell::new(false);
    glib::timeout_add_local(std::time::Duration::from_millis(80), move || {
        let anchor = match cfg_hot.anchor.as_str() {
            "top" => Edge::Top,
            _ => Edge::Bottom,
        };
        let hot = cursor_in_hotzone(&win_hot, anchor)
            || (win_hot.is_visible() && cursor_on_dock(&win_hot, anchor));

        // flag dulu, baru trigger — biar refresh baca state yang bener.
        MOUSE_HOT.store(hot, std::sync::atomic::Ordering::Relaxed);

        if hot && !was_hot.get() {
            // masuk hotzone: munculkan + rebuild isi paling anyar.
            slide(&win_hot, true, anchor, cfg_hot.animation_ms);
            refresh_dock(&dock_hot, &theme_hot, &win_hot, &cfg_hot);
        } else if !hot && was_hot.get() {
            // keluar hotzone: biarkan refresh biasa yang mutusin hide.
            refresh_dock(&dock_hot, &theme_hot, &win_hot, &cfg_hot);
        }

        was_hot.set(hot);
        glib::ControlFlow::Continue
    });

    win.present();
}

/// Flag: cursor lagi di area dock.
static MOUSE_HOT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Nilai true pas dock sembunyi dan cursor dekat area tepi ancor.
fn cursor_in_hotzone(_win: &gtk4::ApplicationWindow, anchor: Edge) -> bool {
    let Some(h) = gtk4::gdk::Display::default() else {
        return false;
    };
    let Some((x, y)) = hypr::cursor_pos() else {
        return false;
    };
    // monitor yang paling deket sama cursor
    let Some(monitor) = h
        .monitors()
        .into_iter()
        .flatten()
        .filter_map(|o| o.downcast::<gtk4::gdk::Monitor>().ok())
        .find(|m| {
            let g = m.geometry();
            x >= g.x() as i64
                && x < (g.x() + g.width()) as i64
                && y >= g.y() as i64
                && y < (g.y() + g.height()) as i64
        })
    else {
        return false;
    };

    const HOTZONE_PX: i64 = 12;
    let g = monitor.geometry();
    match anchor {
        Edge::Bottom => y >= (g.y() + g.height()) as i64 - HOTZONE_PX,
        Edge::Top => y <= g.y() as i64 + HOTZONE_PX,
        _ => false,
    }
}

/// Cursor di atas area dock pas dock visible (biar hover di icon nggak ke-hide).
/// Posisi dock dihitung dari allokasi window + anchor + alignment center.
fn cursor_on_dock(win: &gtk4::ApplicationWindow, anchor: Edge) -> bool {
    let Some(h) = gtk4::gdk::Display::default() else {
        return false;
    };
    let Some((x, y)) = hypr::cursor_pos() else {
        return false;
    };
    // monitor yang paling deket sama cursor
    let Some(monitor) = h
        .monitors()
        .into_iter()
        .flatten()
        .filter_map(|o| o.downcast::<gtk4::gdk::Monitor>().ok())
        .find(|m| {
            let g = m.geometry();
            x >= g.x() as i64
                && x < (g.x() + g.width()) as i64
                && y >= g.y() as i64
                && y < (g.y() + g.height()) as i64
        })
    else {
        return false;
    };

    let g = monitor.geometry();
    let w = win.width().max(1) as i64;
    let hgt = win.height().max(1) as i64;
    let dock_x = g.x() as i64 + ((g.width() as i64 - w) / 2); // halign center
    let dock_y = match anchor {
        Edge::Bottom => (g.y() + g.height()) as i64 - hgt,
        _ => g.y() as i64,
    };

    // kasih margin genap 8px biar tepi icon tetep kena
    x >= dock_x - 8 && x <= dock_x + w + 8 && y >= dock_y - 8 && y <= dock_y + hgt + 8
}

/// Sinyal dari event thread ke main loop.
static EVENT_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Event yang mengubah tampilan dock.
const RELEVANT_EVENTS: &[&str] = &[
    "workspace>>",
    "workspacev2>>",
    "openwindow>>",
    "closewindow>>",
    "activewindow>>",
    "activewindowv2>>",
    "movewindow>>",
    "movewindowv2>>",
    "changefloatingmode>>",
    "fullscreen>>",
    "fullscreenv2>>",
    "createworkspace>>",
    "destroyworkspace>>",
];

fn event_socket_path() -> Option<std::path::PathBuf> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    dirs::runtime_dir()
        .or_else(dirs::data_dir)
        .map(|base| base.join(format!("hypr/{sig}/.socket2.sock")))
}

fn start_event_listener() {
    std::thread::spawn(move || {
        let Some(path) = event_socket_path() else {
            eprintln!("hypr-dock: HYPRLAND_INSTANCE_SIGNATURE tidak ada");
            return;
        };
        let mut stream = match std::os::unix::net::UnixStream::connect(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("hypr-dock: gagal konek event socket {path:?}: {e}");
                return;
            }
        };
        eprintln!("hypr-dock: event socket siap: {path:?}");

        use std::io::Read;
        let mut buf = [0u8; 8192];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break, // socket tutup (Hyprland reload)
                Ok(n) => {
                    let events = String::from_utf8_lossy(&buf[..n]);
                    let hit = events
                        .split('\n')
                        .any(|ev| RELEVANT_EVENTS.iter().any(|p| ev.starts_with(p)));
                    if hit {
                        EVENT_PENDING.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                Err(_) => break,
            }
        }
    });
}

fn clear_box(box_: &gtk4::Box) {
    while let Some(child) = box_.first_child() {
        box_.remove(&child);
    }
}

/// Generasi animasi terbaru. Animasi lama lihat gen-nya basi -> self-stop.
static ANIM_GEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Slide dock masuk/keluar lewat margin layer-shell (ease-out cubic).
fn slide(win: &gtk4::ApplicationWindow, show: bool, anchor: Edge, animation_ms: u64) {
    use gtk4_layer_shell::LayerShell;

    let gen = ANIM_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let start = win.margin(anchor);
    if show && !win.is_visible() {
        win.present();
    }
    let end = if show { 0 } else { offscreen_offset(win) };

    const STEPS: u32 = 8;
    let dt = std::time::Duration::from_millis((animation_ms / STEPS as u64).max(1));
    let mut i = 0u32;
    let win = win.clone();

    // Animasi digantikan = gen beda = Break sendiri, jadi nggak perlu remove SourceId.
    glib::timeout_add_local(dt, move || {
        if ANIM_GEN.load(std::sync::atomic::Ordering::Relaxed) != gen {
            return glib::ControlFlow::Break;
        }
        i += 1;
        let x = (i as f64 / STEPS as f64).min(1.0);
        let ease = 1.0 - (1.0 - x).powi(3);
        let m = (start as f64 + (end as f64 - start as f64) * ease).round() as i32;
        win.set_margin(anchor, m);
        if x >= 1.0 {
            if !show {
                win.set_visible(false);
            }
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

/// Posisi offscreen = negasi tinggi konten + margin keamanan.
fn offscreen_offset(win: &gtk4::ApplicationWindow) -> i32 {
    let h = win.height().max(1);
    -(h + 20)
}

fn refresh_dock(
    dock: &gtk4::Box,
    icon_theme: &gtk4::IconTheme,
    win: &gtk4::ApplicationWindow,
    cfg: &config::ConfigFile,
) {
    let pinned = config::load_pinned();
    let running = hypr::clients();

    // Auto-hide: dock nggak muncul kalo workspace aktif punya app tiling.
    // Kosong atau full floating ("window mode") -> muncul.
    // Cursor di hotzone (mau hover dock) -> tetap muncul.
    let hide_dock = layar_ada_tiling(&running) && !MOUSE_HOT.load(std::sync::atomic::Ordering::Relaxed);

    let anchor = match cfg.anchor.as_str() {
        "top" => Edge::Top,
        _ => Edge::Bottom,
    };

    if hide_dock {
        slide(win, false, anchor, cfg.animation_ms);
        return; // nggak perlu rebuild UI pas animasi keluar
    }
    if !win.is_visible() {
        slide(win, true, anchor, cfg.animation_ms);
    }

    // Rebuild UI cuma pas state berubah — biar hover CSS nggak flashing.
    let sig = state_signature(&pinned, &running);
    let changed = LAST_SIG.with(|c| {
        let mut cur = c.borrow_mut();
        let same = *cur == sig;
        *cur = sig;
        !same
    });
    if !changed {
        return;
    }

    clear_box(dock);

    // pinned dulu (urut config) + dot kalo lagi jalan
    for app in &pinned {
        let opened = running.iter().find(|c| same_class(&c.class, &app.class));
        dock.append(&dock_button(dock, icon_theme, app, opened.cloned(), cfg));
    }

    // app jalan tapi nggak pinned, ditambang di belakang
    for c in &running {
        if pinned.iter().any(|p| same_class(&p.class, &c.class)) {
            continue;
        }
        let launchable = PinnedApp {
            class: c.class.clone(),
            icon: c.class.clone(),
            command: c.class.clone(),
        };
        dock.append(&dock_button(dock, icon_theme, &launchable, Some(c.clone()), cfg));
    }
}

/// True kalo workspace aktif punya client tiling.
fn layar_ada_tiling(running: &[hypr::Client]) -> bool {
    let Some((ws_id, _)) = hypr::active_workspace() else {
        return false;
    };
    running
        .iter()
        .filter(|c| c.workspace_id == ws_id)
        .any(|c| !c.floating)
}

fn same_class(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

thread_local! {
    /// Signature state UI terakhir — dibangun ulang kalo beda.
    static LAST_SIG: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Signature (pinned conf + daftar client) buat deteksi perubahan UI.
fn state_signature(pinned: &[PinnedApp], running: &[hypr::Client]) -> String {
    let mut sig = pinned
        .iter()
        .map(|p| format!("{},{}", p.class, p.command))
        .collect::<Vec<_>>()
        .join("|");
    for c in running {
        sig.push_str(&format!(
            "|{},{},{},{},{}",
            c.address, c.title, c.workspace_id, c.floating, c.special_name.is_some()
        ));
    }
    sig
}

fn dock_button(
    dock: &gtk4::Box,
    icon_theme: &gtk4::IconTheme,
    app: &PinnedApp,
    running: Option<hypr::Client>,
    cfg: &config::ConfigFile,
) -> gtk4::Button {
    let btn = gtk4::Button::builder()
        .tooltip_text(if let Some(c) = &running {
            format!("{} — {}", c.class, c.title)
        } else {
            app.class.clone()
        })
        .build();
    btn.set_widget_name("dock-btn");
    btn.add_css_class("dock-btn");
    if running.is_some() {
        btn.add_css_class("running");
    }
    let icon = hypr::icon_image(icon_theme, &app.class, &app.icon);
    icon.set_pixel_size(cfg.icon_size);
    btn.set_child(Some(&icon));

    btn.connect_clicked(clone_dock_click(dock, app.clone(), running));
    btn
}

/// Klik = kalo jalan focus, kalo nggak spawn.
fn clone_dock_click(
    _dock: &gtk4::Box,
    app: PinnedApp,
    running: Option<hypr::Client>,
) -> impl Fn(&gtk4::Button) + 'static {
    move |_| {
        match &running {
            Some(c) => hypr::focus(c),
            None => hypr::spawn(&app.command),
        };
    }
}
