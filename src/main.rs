mod config;
mod hypr;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4_layer_shell::{Edge, Layer, LayerShell};

use crate::config::PinnedApp;

/// Global CSS theme from external file: ~/.config/hyprland-dock/style.css
/// Uses USER priority to override system GTK themes.

fn main() {
    let app = gtk4::Application::builder()
        .application_id("io.github.anasarifin.hyprlanddock")
        .build();

    app.connect_activate(build_ui);
    std::process::exit(app.run().into());
}

fn build_ui(app: &gtk4::Application) {
    let cfg = std::rc::Rc::new(std::cell::RefCell::new(config::load_toml()));

    let display = gtk4::gdk::Display::default().expect("no display");

    // Built-in base CSS provider loaded from config::DEFAULT_CSS (USER priority overrides ~/.config/gtk-4.0/gtk.css)
    let base_css = gtk4::CssProvider::new();
    base_css.load_from_string(config::DEFAULT_CSS);
    gtk4::style_context_add_provider_for_display(
        &display,
        &base_css,
        gtk4::STYLE_PROVIDER_PRIORITY_USER,
    );

    // Global CSS provider loaded from style.css (user custom overrides)
    let css = gtk4::CssProvider::new();
    css.load_from_string(&config::load_css());
    gtk4::style_context_add_provider_for_display(
        &display,
        &css,
        gtk4::STYLE_PROVIDER_PRIORITY_USER + 1,
    );
    let settings = gtk4::Settings::for_display(&display);
    let theme_name = cfg.borrow().icon_theme.clone();
    if !theme_name.is_empty() {
        settings.set_gtk_icon_theme_name(Some(&theme_name));
    }

    let icon_theme = gtk4::IconTheme::for_display(&display);
    if !theme_name.is_empty() {
        icon_theme.set_theme_name(Some(&theme_name));
    }

    let win = gtk4::ApplicationWindow::builder()
        .application(app)
        .resizable(false)
        .decorated(false)
        .default_width(100)
        .build();
    win.add_css_class("dock-window");

    // ---- layer shell setup ----
    if !gtk4_layer_shell::is_supported() {
        eprintln!("hyprland-dock: compositor does not support layer-shell");
        std::process::exit(1);
    }
    let anchor_edge = match cfg.borrow().anchor.as_str() {
        "top" => Edge::Top,
        _ => Edge::Bottom,
    };
    win.init_layer_shell();
    win.set_layer(Layer::Top);
    win.set_namespace(Some("hyprland-dock"));
    hypr::setup_layer_rules();
    hypr::setup_window_rules();
    win.set_anchor(anchor_edge, true);
    win.set_exclusive_zone(-1); // -1 = match content height so windows do not overlap

    // ---- dock contents ----
    let outer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    outer.set_halign(gtk4::Align::Center);
    outer.set_hexpand(true);
    outer.set_overflow(gtk4::Overflow::Visible);
    match cfg.borrow().anchor.as_str() {
        "top" => outer.set_margin_bottom(16),
        _ => outer.set_margin_top(16),
    }

    let dock = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    dock.set_widget_name("dock");
    dock.add_css_class("dock");
    dock.set_overflow(gtk4::Overflow::Visible);
    match cfg.borrow().anchor.as_str() {
        "top" => dock.add_css_class("anchor-top"),
        _ => dock.add_css_class("anchor-bottom"),
    }
    outer.append(&dock);
    win.set_child(Some(&outer));
    win.set_overflow(gtk4::Overflow::Visible);

    refresh_dock(&dock, &icon_theme, &win, &cfg);

    // Backup poll at 1s interval (Hyprland IPC events trigger instant updates)
    let dock_ref = dock.clone();
    let theme_ref = icon_theme.clone();
    let win_ref = win.clone();
    let cfg_ref = cfg.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(1000), move || {
        refresh_dock(&dock_ref, &theme_ref, &win_ref, &cfg_ref);
        glib::ControlFlow::Continue
    });

    // Event thread sets PENDING flag; this 25ms loop performs the refresh
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

    // Hotzone: reveals dock when cursor approaches screen edge while dock is hidden
    let dock_hot = dock.clone();
    let theme_hot = icon_theme.clone();
    let win_hot = win.clone();
    let cfg_hot = cfg.clone();
    let was_hot = std::cell::Cell::new(false);
    glib::timeout_add_local(std::time::Duration::from_millis(80), move || {
        let anchor = match cfg_hot.borrow().anchor.as_str() {
            "top" => Edge::Top,
            _ => Edge::Bottom,
        };
        let anim_ms = cfg_hot.borrow().animation_ms;
        let hot = cursor_in_hotzone(&win_hot, anchor)
            || (win_hot.is_visible() && cursor_on_dock(&win_hot, anchor))
            || is_menu_open();

        // Update flag before triggering refresh so proper state is read
        MOUSE_HOT.store(hot, std::sync::atomic::Ordering::Relaxed);

        if hot && !was_hot.get() {
            // Entered hotzone: slide in and rebuild with latest state
            slide(&win_hot, true, anchor, anim_ms);
            refresh_dock(&dock_hot, &theme_hot, &win_hot, &cfg_hot);
        } else if !hot && was_hot.get() {
            // Left hotzone: let standard refresh logic determine hiding
            refresh_dock(&dock_hot, &theme_hot, &win_hot, &cfg_hot);
        }

        was_hot.set(hot);
        glib::ControlFlow::Continue
    });

    win.present();
}

/// Flag indicating cursor is over the dock area.
static MOUSE_HOT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Number of currently active popover menus.
static MENU_OPEN_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Flag indicating a drag-and-drop reorder is in progress.
static IS_DRAGGING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn is_menu_open() -> bool {
    MENU_OPEN_COUNT.load(std::sync::atomic::Ordering::Relaxed) > 0
}

/// Returns true when dock is hidden and cursor is near the anchor edge.
fn cursor_in_hotzone(_win: &gtk4::ApplicationWindow, anchor: Edge) -> bool {
    let Some(h) = gtk4::gdk::Display::default() else {
        return false;
    };
    let Some((x, y)) = hypr::cursor_pos() else {
        return false;
    };
    // Find monitor containing cursor
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

/// Cursor is over dock while visible (prevents hiding when hovering icons).
/// Dock coordinates calculated from window geometry, anchor, and center alignment.
fn cursor_on_dock(win: &gtk4::ApplicationWindow, anchor: Edge) -> bool {
    let Some(h) = gtk4::gdk::Display::default() else {
        return false;
    };
    let Some((x, y)) = hypr::cursor_pos() else {
        return false;
    };
    // Find monitor containing cursor
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

    // 8px margin buffer to ensure icon edges are reliably reached
    x >= dock_x - 8 && x <= dock_x + w + 8 && y >= dock_y - 8 && y <= dock_y + hgt + 8
}

/// Signal from IPC event listener to main loop.
static EVENT_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Events that trigger dock updates.
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
            eprintln!("hyprland-dock: HYPRLAND_INSTANCE_SIGNATURE not found");
            return;
        };
        let mut stream = match std::os::unix::net::UnixStream::connect(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("hyprland-dock: failed to connect to event socket {path:?}: {e}");
                return;
            }
        };
        eprintln!("hyprland-dock: event socket connected: {path:?}");

        use std::io::Read;
        let mut buf = [0u8; 8192];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break, // socket closed (Hyprland reloaded)
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

/// Latest animation generation token; stale animations self-terminate.
static ANIM_GEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Slide dock in/out using layer-shell margins with ease-out cubic curve.
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

    // Superseded animations detect token mismatch and break automatically
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

/// Offscreen position equals negative content height plus safety margin.
fn offscreen_offset(win: &gtk4::ApplicationWindow) -> i32 {
    let h = win.height().max(1);
    -(h + 20)
}

fn refresh_dock(
    dock: &gtk4::Box,
    icon_theme: &gtk4::IconTheme,
    win: &gtk4::ApplicationWindow,
    cfg: &std::rc::Rc<std::cell::RefCell<config::ConfigFile>>,
) {
    if is_menu_open() {
        return;
    }

    let pinned = config::load_pinned();
    let running = hypr::clients();
    let active_addr = hypr::active_window_address();
    let is_locked = cfg.borrow().locked;
    let anchor_str = cfg.borrow().anchor.clone();
    let anim_ms = cfg.borrow().animation_ms;

    // Auto-hide: dock hides when active workspace has tiled windows.
    // Empty or full floating ("window mode") -> shown.
    // Cursor in hotzone / menu open -> stays shown.
    let has_tiling = workspace_has_tiling(&running);
    let hide_dock =
        has_tiling && !MOUSE_HOT.load(std::sync::atomic::Ordering::Relaxed) && !is_menu_open();

    let anchor = match anchor_str.as_str() {
        "top" => Edge::Top,
        _ => Edge::Bottom,
    };

    // Toggle background styling when on a workspace with tiled windows
    if has_tiling {
        if !dock.has_css_class("has-window") {
            dock.add_css_class("has-window");
        }
    } else if dock.has_css_class("has-window") {
        dock.remove_css_class("has-window");
    }

    if hide_dock {
        slide(win, false, anchor, anim_ms);
        return; // skip UI rebuild during exit animation
    }
    if !win.is_visible() {
        slide(win, true, anchor, anim_ms);
    }

    // Rebuild UI only when state changes to avoid hover CSS flickering
    let sig = state_signature(&pinned, &running, active_addr.as_deref(), is_locked);
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

    // Pinned apps first (ordered by config)
    for app in &pinned {
        let matching: Vec<&hypr::Client> = running
            .iter()
            .filter(|c| same_class(&c.class, &app.class))
            .collect();

        if matching.is_empty() {
            // Pinned app with no open window: show 1 launcher button
            dock.append(&dock_button(dock, icon_theme, app, None, false, cfg));
        } else {
            // Multiple windows open: show one icon per open window
            for c in matching {
                let is_active = active_addr.as_deref() == Some(&c.address);
                dock.append(&dock_button(
                    dock,
                    icon_theme,
                    app,
                    Some((*c).clone()),
                    is_active,
                    cfg,
                ));
            }
        }
    }

    // Running apps that are not pinned: each open window gets an icon
    for c in &running {
        if pinned.iter().any(|p| same_class(&p.class, &c.class)) {
            continue;
        }
        let (icon, cmd) = config::resolve_app_info(&c.class, c.pid);
        let launchable = PinnedApp {
            class: c.class.clone(),
            label: display_class_name(&c.class),
            icon,
            command: cmd,
        };
        let is_active = active_addr.as_deref() == Some(&c.address);
        dock.append(&dock_button(
            dock,
            icon_theme,
            &launchable,
            Some(c.clone()),
            is_active,
            cfg,
        ));
    }
}

/// True if active workspace has at least one tiled client.
fn workspace_has_tiling(running: &[hypr::Client]) -> bool {
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

fn display_class_name(class: &str) -> String {
    class
        .split('.')
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or(class)
        .to_lowercase()
}

thread_local! {
    /// Last known UI state signature; rebuilt when different.
    static LAST_SIG: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// UI state signature (pinned list + clients + lock status) for change detection.
fn state_signature(
    pinned: &[PinnedApp],
    running: &[hypr::Client],
    active_addr: Option<&str>,
    locked: bool,
) -> String {
    let mut sig = format!("locked:{}|active:{}|", locked, active_addr.unwrap_or(""));
    for p in pinned {
        sig.push_str(&format!(
            "{},{},{},{}|",
            p.class, p.command, p.label, p.icon
        ));
    }
    for c in running {
        sig.push_str(&format!(
            "|{},{},{},{},{}",
            c.address,
            c.title,
            c.workspace_id,
            c.floating,
            c.special_name.is_some()
        ));
    }
    sig
}

fn dock_button(
    dock: &gtk4::Box,
    icon_theme: &gtk4::IconTheme,
    app: &PinnedApp,
    running: Option<hypr::Client>,
    is_active: bool,
    cfg: &std::rc::Rc<std::cell::RefCell<config::ConfigFile>>,
) -> gtk4::Button {
    let tooltip = if let Some(c) = &running {
        let name = if !app.label.is_empty() {
            &app.label
        } else {
            &display_class_name(&c.class)
        };
        if c.title.is_empty() {
            name.to_string()
        } else {
            format!("{} — {}", name, c.title)
        }
    } else {
        if !app.label.is_empty() {
            app.label.clone()
        } else {
            display_class_name(&app.class)
        }
    };

    let btn = gtk4::Button::new();
    btn.set_widget_name("dock-btn");
    btn.add_css_class("dock-btn");
    btn.set_cursor_from_name(Some("pointer"));
    btn.set_overflow(gtk4::Overflow::Visible);
    if running.is_some() {
        btn.add_css_class("running");
    }
    if is_active {
        btn.add_css_class("active");
    }
    let icon = hypr::icon_image(icon_theme, &app.class, &app.icon);
    icon.set_pixel_size(cfg.borrow().icon_size);
    btn.set_child(Some(&icon));

    let hover_popover = setup_hover_tooltip(&btn, &tooltip, running.as_ref(), cfg);
    btn.connect_clicked(clone_dock_click(
        dock,
        app.clone(),
        running.clone(),
        hover_popover.clone(),
    ));
    setup_context_menu(&btn, icon_theme, app, running.as_ref(), &hover_popover, cfg);
    setup_drag_and_drop(&btn, app, &hover_popover, cfg);

    btn
}

/// Floating title tooltip anchored directly above (or below) the icon button instead of tracking cursor.
/// If window_preview is enabled and the window is running, debounces 500ms before capturing and displaying
/// an async thumbnail preview of the window.
fn setup_hover_tooltip(
    btn: &gtk4::Button,
    text: &str,
    running: Option<&hypr::Client>,
    cfg: &std::rc::Rc<std::cell::RefCell<config::ConfigFile>>,
) -> gtk4::Popover {
    let popover = gtk4::Popover::builder()
        .has_arrow(true)
        .autohide(false)
        .can_target(false)
        .position(match cfg.borrow().anchor.as_str() {
            "top" => gtk4::PositionType::Bottom,
            _ => gtk4::PositionType::Top,
        })
        .build();
    popover.add_css_class("dock-tooltip");
    popover.set_parent(btn);

    let container = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    container.set_halign(gtk4::Align::Center);

    let picture = gtk4::Picture::new();
    picture.add_css_class("dock-tooltip-preview");
    picture.set_content_fit(gtk4::ContentFit::ScaleDown);
    picture.set_can_shrink(true);
    picture.set_visible(false);
    container.append(&picture);

    let preview_width = cfg.borrow().window_preview_width.max(120);
    let label = gtk4::Label::new(Some(text));
    label.add_css_class("dock-tooltip-label");
    label.set_halign(gtk4::Align::Center);
    label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    let max_chars = ((preview_width as f64) / 8.5).round() as i32;
    label.set_max_width_chars(max_chars.clamp(16, 100));
    container.append(&label);

    popover.set_child(Some(&container));

    let running_address = running.map(|c| c.address.clone());
    let debounce_source: std::rc::Rc<std::cell::RefCell<Option<glib::SourceId>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let is_hovered = std::rc::Rc::new(std::cell::Cell::new(false));

    let motion = gtk4::EventControllerMotion::new();

    let popover_enter = popover.clone();
    let picture_enter = picture.clone();
    let cfg_enter = cfg.clone();
    let debounce_enter = debounce_source.clone();
    let is_hovered_enter = is_hovered.clone();

    motion.connect_enter(move |_, _, _| {
        if is_menu_open() || IS_DRAGGING.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }

        is_hovered_enter.set(true);

        // Cancel any pending timer
        if let Some(source) = debounce_enter.borrow_mut().take() {
            source.remove();
        }

        let preview_enabled = cfg_enter.borrow().window_preview;
        let preview_w = cfg_enter.borrow().window_preview_width.max(120);
        let delay_ms = cfg_enter.borrow().window_preview_delay;

        if let (true, Some(addr)) = (preview_enabled, running_address.clone()) {
            let popover_debounced = popover_enter.clone();
            let picture_debounced = picture_enter.clone();
            let is_hovered_debounced = is_hovered_enter.clone();
            let debounce_ref = debounce_enter.clone();

            let source_id = glib::timeout_add_local_once(
                std::time::Duration::from_millis(delay_ms),
                move || {
                    debounce_ref.borrow_mut().take();
                    if !is_hovered_debounced.get()
                        || is_menu_open()
                        || IS_DRAGGING.load(std::sync::atomic::Ordering::Relaxed)
                    {
                        return;
                    }

                    let picture_load = picture_debounced.clone();
                    let popover_load = popover_debounced.clone();
                    let is_hovered_load = is_hovered_debounced.clone();

                    let (tx, rx) = std::sync::mpsc::channel::<Option<Vec<u8>>>();
                    std::thread::spawn(move || {
                        let jpeg_opt = hypr::capture_window_preview(&addr);
                        let _ = tx.send(jpeg_opt);
                    });

                    let mut attempts = 0;
                    glib::timeout_add_local(std::time::Duration::from_millis(25), move || {
                        attempts += 1;
                        if !is_hovered_load.get()
                            || is_menu_open()
                            || IS_DRAGGING.load(std::sync::atomic::Ordering::Relaxed)
                        {
                            return glib::ControlFlow::Break;
                        }

                        match rx.try_recv() {
                            Ok(jpeg_opt) => {
                                if let Some(bytes) = jpeg_opt {
                                    if let Some(texture) =
                                        hypr::create_preview_texture(&bytes, preview_w)
                                    {
                                        picture_load.set_paintable(Some(&texture));
                                        picture_load.set_visible(true);
                                    } else {
                                        picture_load.set_visible(false);
                                    }
                                } else {
                                    picture_load.set_visible(false);
                                }
                                popover_load.popup();
                                glib::ControlFlow::Break
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {
                                if attempts > 40 {
                                    glib::ControlFlow::Break
                                } else {
                                    glib::ControlFlow::Continue
                                }
                            }
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                glib::ControlFlow::Break
                            }
                        }
                    });
                },
            );
            *debounce_enter.borrow_mut() = Some(source_id);
        } else {
            picture_enter.set_visible(false);
            popover_enter.popup();
        }
    });

    let popover_leave = popover.clone();
    let picture_leave = picture.clone();
    let debounce_leave = debounce_source.clone();
    let is_hovered_leave = is_hovered.clone();

    motion.connect_leave(move |_| {
        is_hovered_leave.set(false);
        if let Some(source) = debounce_leave.borrow_mut().take() {
            source.remove();
        }
        picture_leave.set_visible(false);
        picture_leave.set_paintable(None::<&gtk4::gdk::Paintable>);
        popover_leave.popdown();
    });

    let picture_closed = picture.clone();
    let debounce_closed = debounce_source.clone();
    let is_hovered_closed = is_hovered.clone();
    popover.connect_closed(move |_| {
        is_hovered_closed.set(false);
        if let Some(source) = debounce_closed.borrow_mut().take() {
            source.remove();
        }
        picture_closed.set_visible(false);
        picture_closed.set_paintable(None::<&gtk4::gdk::Paintable>);
    });

    btn.add_controller(motion);
    popover
}

fn setup_context_menu(
    btn: &gtk4::Button,
    icon_theme: &gtk4::IconTheme,
    app: &PinnedApp,
    running: Option<&hypr::Client>,
    hover_popover: &gtk4::Popover,
    cfg: &std::rc::Rc<std::cell::RefCell<config::ConfigFile>>,
) {
    let popover = gtk4::Popover::builder()
        .has_arrow(true)
        .autohide(true)
        .position(match cfg.borrow().anchor.as_str() {
            "top" => gtk4::PositionType::Bottom,
            _ => gtk4::PositionType::Top,
        })
        .build();
    popover.add_css_class("dock-popover");
    popover.set_parent(btn);

    let menu_box = gtk4::Box::new(gtk4::Orientation::Vertical, 2);

    let title_text = if let Some(c) = running {
        if !app.label.is_empty() {
            if c.title.is_empty() {
                app.label.clone()
            } else {
                c.title.clone()
            }
        } else if c.title.is_empty() {
            display_class_name(&c.class)
        } else {
            c.title.clone()
        }
    } else {
        if !app.label.is_empty() {
            app.label.clone()
        } else {
            display_class_name(&app.class)
        }
    };
    let title_label = gtk4::Label::new(Some(&title_text));
    title_label.add_css_class("dock-menu-title");
    title_label.set_halign(gtk4::Align::Fill);
    title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    title_label.set_max_width_chars(25);
    menu_box.append(&title_label);

    let sep = gtk4::Separator::new(gtk4::Orientation::Horizontal);
    sep.add_css_class("dock-menu-sep");
    menu_box.append(&sep);

    // If running, provide New Window and Close Window options
    if let Some(c) = running {
        let new_win_btn = gtk4::Button::builder()
            .label("New Window")
            .css_classes(["dock-menu-btn"])
            .halign(gtk4::Align::Fill)
            .build();
        new_win_btn.set_cursor_from_name(Some("pointer"));
        let popover_new = popover.clone();
        let cmd = app.command.clone();
        new_win_btn.connect_clicked(move |_| {
            popover_new.popdown();
            hypr::spawn(&cmd);
        });
        menu_box.append(&new_win_btn);

        let close_win_btn = gtk4::Button::builder()
            .label("Close Window")
            .css_classes(["dock-menu-btn"])
            .halign(gtk4::Align::Fill)
            .build();
        close_win_btn.set_cursor_from_name(Some("pointer"));
        let popover_close = popover.clone();
        let addr = c.address.clone();
        close_win_btn.connect_clicked(move |_| {
            popover_close.popdown();
            hypr::close_window(&addr);
            EVENT_PENDING.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        menu_box.append(&close_win_btn);

        let sep2 = gtk4::Separator::new(gtk4::Orientation::Horizontal);
        sep2.add_css_class("dock-menu-sep");
        menu_box.append(&sep2);
    }

    let is_locked = cfg.borrow().locked;

    // Edit Icon option
    let edit_btn = gtk4::Button::builder()
        .label("Edit Icon")
        .css_classes(["dock-menu-btn"])
        .halign(gtk4::Align::Fill)
        .sensitive(!is_locked)
        .build();
    if !is_locked {
        edit_btn.set_cursor_from_name(Some("pointer"));
    }

    let popover_edit = popover.clone();
    let app_edit = app.clone();
    let theme_edit = icon_theme.clone();
    edit_btn.connect_clicked(move |_| {
        popover_edit.popdown();
        show_edit_dialog(&app_edit, &theme_edit);
    });
    menu_box.append(&edit_btn);

    // Pin / Unpin option
    let is_pinned = config::load_pinned()
        .iter()
        .any(|p| same_class(&p.class, &app.class));
    let pin_label = if is_pinned {
        "Unpin from Dock"
    } else {
        "Pin to Dock"
    };
    let pin_btn = gtk4::Button::builder()
        .label(pin_label)
        .css_classes(["dock-menu-btn"])
        .halign(gtk4::Align::Fill)
        .sensitive(!is_locked)
        .build();
    if !is_locked {
        pin_btn.set_cursor_from_name(Some("pointer"));
    }

    let popover_pin = popover.clone();
    let app_pin = app.clone();
    pin_btn.connect_clicked(move |_| {
        popover_pin.popdown();
        let mut pinned = config::load_pinned();
        if is_pinned {
            pinned.retain(|p| !same_class(&p.class, &app_pin.class));
        } else {
            let mut to_pin = app_pin.clone();
            if to_pin.label.is_empty() {
                to_pin.label = display_class_name(&to_pin.class);
            }
            if !config::binary_exists(&to_pin.command) {
                let (icon, cmd) = config::resolve_app_info(&to_pin.class, None);
                if config::binary_exists(&cmd) {
                    to_pin.command = cmd;
                    if to_pin.icon == to_pin.class || !config::binary_exists(&to_pin.icon) {
                        to_pin.icon = icon;
                    }
                }
            }
            if to_pin.icon.is_empty() {
                let (icon, _) = config::resolve_app_info(&to_pin.class, None);
                to_pin.icon = icon;
            }
            if !pinned.iter().any(|p| same_class(&p.class, &to_pin.class)) {
                pinned.push(to_pin);
            }
        }
        let _ = config::save_pinned(&pinned);
        EVENT_PENDING.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    menu_box.append(&pin_btn);

    // Lock / Unlock option
    let lock_label = if is_locked {
        "Unlock Dock"
    } else {
        "Lock Dock"
    };
    let lock_btn = gtk4::Button::builder()
        .label(lock_label)
        .css_classes(["dock-menu-btn"])
        .halign(gtk4::Align::Fill)
        .build();
    lock_btn.set_cursor_from_name(Some("pointer"));

    let popover_lock = popover.clone();
    let cfg_lock = cfg.clone();
    lock_btn.connect_clicked(move |_| {
        popover_lock.popdown();
        let new_locked = !cfg_lock.borrow().locked;
        cfg_lock.borrow_mut().locked = new_locked;
        let _ = config::save_toml(&cfg_lock.borrow());
        EVENT_PENDING.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    menu_box.append(&lock_btn);

    popover.set_child(Some(&menu_box));

    let click_gesture = gtk4::GestureClick::new();
    click_gesture.set_button(gtk4::gdk::BUTTON_SECONDARY);
    let popover_click = popover.clone();
    let hover_close = hover_popover.clone();
    click_gesture.connect_pressed(move |_, _, _, _| {
        hover_close.popdown();
        MENU_OPEN_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        popover_click.popup();
    });
    btn.add_controller(click_gesture);

    popover.connect_closed(move |_| {
        MENU_OPEN_COUNT.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        EVENT_PENDING.store(true, std::sync::atomic::Ordering::Relaxed);
    });
}

/// Modal dialog to edit launcher command, label, and icon (class is read-only).
fn show_edit_dialog(app: &PinnedApp, icon_theme: &gtk4::IconTheme) {
    let window = gtk4::Window::builder()
        .title("Edit Icon")
        .default_width(400)
        .resizable(false)
        .modal(false)
        .build();
    window.add_css_class("dock-edit-window");

    MENU_OPEN_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    window.connect_close_request(|_| {
        MENU_OPEN_COUNT.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        EVENT_PENDING.store(true, std::sync::atomic::Ordering::Relaxed);
        glib::Propagation::Proceed
    });

    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 14);
    root.set_margin_top(20);
    root.set_margin_bottom(20);
    root.set_margin_start(20);
    root.set_margin_end(20);

    // Header with current icon preview and title
    let header_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 14);
    header_box.set_halign(gtk4::Align::Start);
    header_box.set_valign(gtk4::Align::Center);

    let icon_preview = hypr::icon_image(icon_theme, &app.class, &app.icon);
    icon_preview.set_pixel_size(48);
    icon_preview.set_cursor_from_name(Some("pointer"));
    icon_preview.set_tooltip_text(Some("Click to browse theme icons"));
    header_box.append(&icon_preview);

    let header_text_box = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    let title = gtk4::Label::builder()
        .label("Edit Icon")
        .halign(gtk4::Align::Start)
        .css_classes(["dock-edit-title"])
        .build();
    let subtitle = gtk4::Label::builder()
        .label(&format!("Configure settings for {}", app.class))
        .halign(gtk4::Align::Start)
        .css_classes(["dock-edit-subtitle"])
        .build();
    header_text_box.append(&title);
    header_text_box.append(&subtitle);
    header_box.append(&header_text_box);
    root.append(&header_box);

    let sep = gtk4::Separator::new(gtk4::Orientation::Horizontal);
    root.append(&sep);

    // Form fields
    let form = gtk4::Box::new(gtk4::Orientation::Vertical, 10);

    // Field 1: Class (Read-only, cannot be edited)
    let class_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    let class_lbl = gtk4::Label::builder()
        .label("Class")
        .halign(gtk4::Align::Start)
        .css_classes(["dock-edit-label"])
        .build();
    let class_entry = gtk4::Entry::builder()
        .text(&app.class)
        .editable(false)
        .can_focus(false)
        .sensitive(false)
        .css_classes(["dock-edit-entry", "disabled"])
        .build();
    class_box.append(&class_lbl);
    class_box.append(&class_entry);
    form.append(&class_box);

    // Field 2: Command
    let cmd_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    let cmd_lbl = gtk4::Label::builder()
        .label("Command")
        .halign(gtk4::Align::Start)
        .css_classes(["dock-edit-label"])
        .build();
    let cmd_entry = gtk4::Entry::builder()
        .text(&app.command)
        .placeholder_text("e.g. firefox")
        .css_classes(["dock-edit-entry"])
        .build();
    cmd_box.append(&cmd_lbl);
    cmd_box.append(&cmd_entry);
    form.append(&cmd_box);

    // Field 3: Label
    let label_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    let label_lbl = gtk4::Label::builder()
        .label("Label")
        .halign(gtk4::Align::Start)
        .css_classes(["dock-edit-label"])
        .build();
    let label_entry = gtk4::Entry::builder()
        .text(&app.label)
        .placeholder_text("e.g. Firefox Browser")
        .css_classes(["dock-edit-entry"])
        .build();
    label_box.append(&label_lbl);
    label_box.append(&label_entry);
    form.append(&label_box);

    // Field 4: Icon (with Browse Theme button)
    let icon_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    let icon_lbl = gtk4::Label::builder()
        .label("Icon (leave empty for app default)")
        .halign(gtk4::Align::Start)
        .css_classes(["dock-edit-label"])
        .build();

    let icon_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let icon_entry = gtk4::Entry::builder()
        .text(&app.icon)
        .placeholder_text("e.g. firefox")
        .css_classes(["dock-edit-entry"])
        .hexpand(true)
        .build();

    let browse_btn = gtk4::Button::builder()
        .label("Browse Icons")
        .css_classes(["dock-picker-browse-btn"])
        .build();
    browse_btn.set_cursor_from_name(Some("pointer"));
    browse_btn.set_tooltip_text(Some("Pick icon directly from active theme"));

    icon_row.append(&icon_entry);
    icon_row.append(&browse_btn);

    icon_box.append(&icon_lbl);
    icon_box.append(&icon_row);
    form.append(&icon_box);

    root.append(&form);

    // Live preview update on typing
    let icon_preview_clone = icon_preview.clone();
    let class_clone = app.class.clone();
    let theme_clone = icon_theme.clone();
    icon_entry.connect_changed(move |entry| {
        let typed = entry.text().trim().to_string();
        let name = hypr::resolve_icon_name(&theme_clone, &class_clone, &typed);
        icon_preview_clone.set_icon_name(Some(&name));
    });

    // Open icon picker from Browse button
    let win_picker = window.clone();
    let theme_picker = icon_theme.clone();
    let class_picker = app.class.clone();
    let entry_picker = icon_entry.clone();
    let preview_picker = icon_preview.clone();
    browse_btn.connect_clicked(move |_| {
        show_icon_picker(
            &win_picker,
            &theme_picker,
            &class_picker,
            &entry_picker,
            &preview_picker,
        );
    });

    // Also clicking the icon preview opens the icon picker
    let preview_click = gtk4::GestureClick::new();
    let win_picker2 = window.clone();
    let theme_picker2 = icon_theme.clone();
    let class_picker2 = app.class.clone();
    let entry_picker2 = icon_entry.clone();
    let preview_picker2 = icon_preview.clone();
    preview_click.connect_pressed(move |_, _, _, _| {
        show_icon_picker(
            &win_picker2,
            &theme_picker2,
            &class_picker2,
            &entry_picker2,
            &preview_picker2,
        );
    });
    icon_preview.add_controller(preview_click);

    // Action buttons (Cancel / Save)
    let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    btn_box.set_halign(gtk4::Align::End);
    btn_box.set_margin_top(8);

    let cancel_btn = gtk4::Button::builder()
        .label("Cancel")
        .css_classes(["dock-edit-btn", "cancel"])
        .build();
    cancel_btn.set_cursor_from_name(Some("pointer"));
    let win_cancel = window.clone();
    cancel_btn.connect_clicked(move |_| {
        win_cancel.close();
    });
    btn_box.append(&cancel_btn);

    let save_btn = gtk4::Button::builder()
        .label("Save")
        .css_classes(["dock-edit-btn", "save"])
        .build();
    save_btn.set_cursor_from_name(Some("pointer"));

    let win_save = window.clone();
    let app_target = app.clone();
    let cmd_entry_save = cmd_entry.clone();
    let label_entry_save = label_entry.clone();
    let icon_entry_save = icon_entry.clone();

    save_btn.connect_clicked(move |_| {
        let new_cmd = cmd_entry_save.text().trim().to_string();
        let new_label = label_entry_save.text().trim().to_string();
        let new_icon = icon_entry_save.text().trim().to_string();

        let final_cmd = if new_cmd.is_empty() {
            app_target.command.clone()
        } else {
            new_cmd
        };

        let mut pinned = config::load_pinned();
        if let Some(pos) = pinned
            .iter()
            .position(|p| same_class(&p.class, &app_target.class))
        {
            pinned[pos].command = final_cmd;
            pinned[pos].label = new_label;
            pinned[pos].icon = new_icon;
        } else {
            pinned.push(PinnedApp {
                class: app_target.class.clone(),
                command: final_cmd,
                label: new_label,
                icon: new_icon,
            });
        }

        let _ = config::save_pinned(&pinned);
        EVENT_PENDING.store(true, std::sync::atomic::Ordering::Relaxed);
        win_save.close();
    });
    btn_box.append(&save_btn);

    let save_btn_enter1 = save_btn.clone();
    cmd_entry.connect_activate(move |_| save_btn_enter1.emit_clicked());
    let save_btn_enter2 = save_btn.clone();
    label_entry.connect_activate(move |_| save_btn_enter2.emit_clicked());
    let save_btn_enter3 = save_btn.clone();
    icon_entry.connect_activate(move |_| save_btn_enter3.emit_clicked());

    // ESC key to close
    let key_controller = gtk4::EventControllerKey::new();
    let win_key = window.clone();
    key_controller.connect_key_pressed(move |_, key, _, _| {
        if key == gtk4::gdk::Key::Escape {
            win_key.close();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(key_controller);

    root.append(&btn_box);
    window.set_child(Some(&root));

    hypr::setup_window_rules();
    window.present();
}

/// Modal icon picker to browse and select icons directly from the active GTK icon theme.
fn show_icon_picker(
    parent: &gtk4::Window,
    icon_theme: &gtk4::IconTheme,
    app_class: &str,
    target_entry: &gtk4::Entry,
    target_preview: &gtk4::Image,
) {
    let window = gtk4::Window::builder()
        .title("Select Icon")
        .default_width(620)
        .default_height(480)
        .resizable(false)
        .modal(false)
        .transient_for(parent)
        .build();
    window.add_css_class("dock-picker-window");

    MENU_OPEN_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    window.connect_close_request(|_| {
        MENU_OPEN_COUNT.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        EVENT_PENDING.store(true, std::sync::atomic::Ordering::Relaxed);
        glib::Propagation::Proceed
    });

    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(16);
    root.set_margin_end(16);

    let header_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let title = gtk4::Label::builder()
        .label("Select Icon from Theme")
        .halign(gtk4::Align::Start)
        .css_classes(["dock-edit-title"])
        .build();
    header_box.append(&title);
    root.append(&header_box);

    let search_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let search_entry = gtk4::SearchEntry::builder()
        .placeholder_text("Search icons (e.g. firefox, terminal, player)...")
        .css_classes(["dock-picker-search"])
        .hexpand(true)
        .build();
    search_box.append(&search_entry);
    root.append(&search_box);

    let count_lbl = gtk4::Label::builder()
        .label("Loading icons...")
        .halign(gtk4::Align::Start)
        .css_classes(["dock-picker-count"])
        .build();
    root.append(&count_lbl);

    let scrolled = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .css_classes(["dock-picker-scroll"])
        .vexpand(true)
        .build();

    let flowbox = gtk4::FlowBox::builder()
        .selection_mode(gtk4::SelectionMode::None)
        .valign(gtk4::Align::Start)
        .min_children_per_line(4)
        .max_children_per_line(7)
        .homogeneous(true)
        .row_spacing(8)
        .column_spacing(8)
        .css_classes(["dock-picker-flowbox"])
        .build();

    scrolled.set_child(Some(&flowbox));
    root.append(&scrolled);

    let bottom_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    bottom_box.set_halign(gtk4::Align::End);

    let cancel_btn = gtk4::Button::builder()
        .label("Cancel")
        .css_classes(["dock-edit-btn", "cancel"])
        .build();
    cancel_btn.set_cursor_from_name(Some("pointer"));
    let win_cancel = window.clone();
    cancel_btn.connect_clicked(move |_| {
        win_cancel.close();
    });
    bottom_box.append(&cancel_btn);
    root.append(&bottom_box);

    window.set_child(Some(&root));

    // ESC to close
    let key_controller = gtk4::EventControllerKey::new();
    let win_key = window.clone();
    key_controller.connect_key_pressed(move |_, key, _, _| {
        if key == gtk4::gdk::Key::Escape {
            win_key.close();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(key_controller);

    // Cache sorted icon names from active theme
    let all_icons: std::rc::Rc<Vec<String>> = {
        let mut list: Vec<String> = icon_theme
            .icon_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        list.sort();
        list.dedup();
        std::rc::Rc::new(list)
    };

    let populate_grid = {
        let flowbox = flowbox.clone();
        let count_lbl = count_lbl.clone();
        let all_icons = all_icons.clone();
        let target_entry = target_entry.clone();
        let target_preview = target_preview.clone();
        let icon_theme = icon_theme.clone();
        let app_class = app_class.to_string();
        let window = window.clone();

        move |query: &str| {
            while let Some(child) = flowbox.first_child() {
                flowbox.remove(&child);
            }

            let q = query.trim().to_lowercase();
            let matches: Vec<&String> = if q.is_empty() {
                all_icons.iter().take(120).collect()
            } else {
                all_icons
                    .iter()
                    .filter(|name| name.to_lowercase().contains(&q))
                    .take(120)
                    .collect()
            };

            let total_matches = if q.is_empty() {
                all_icons.len()
            } else {
                all_icons
                    .iter()
                    .filter(|name| name.to_lowercase().contains(&q))
                    .count()
            };

            if q.is_empty() {
                count_lbl.set_label(&format!(
                    "Showing {} of {} icons (type to filter)",
                    matches.len(),
                    all_icons.len()
                ));
            } else {
                count_lbl.set_label(&format!(
                    "Found {} icons (showing top {})",
                    total_matches,
                    matches.len()
                ));
            }

            for icon_name in matches {
                let btn = gtk4::Button::new();
                btn.add_css_class("dock-icon-card");
                btn.set_cursor_from_name(Some("pointer"));
                btn.set_tooltip_text(Some(icon_name));
                btn.set_valign(gtk4::Align::Start);
                btn.set_size_request(82, 82);

                let item_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
                item_box.set_halign(gtk4::Align::Center);
                item_box.set_valign(gtk4::Align::Center);
                item_box.set_margin_top(4);
                item_box.set_margin_bottom(4);
                item_box.set_margin_start(4);
                item_box.set_margin_end(4);

                let img = gtk4::Image::from_icon_name(icon_name);
                img.set_pixel_size(40);
                item_box.append(&img);

                let lbl = gtk4::Label::new(Some(icon_name));
                lbl.add_css_class("dock-icon-card-label");
                lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                lbl.set_max_width_chars(11);
                item_box.append(&lbl);

                btn.set_child(Some(&item_box));

                let chosen = icon_name.clone();
                let target_entry_c = target_entry.clone();
                let target_preview_c = target_preview.clone();
                let win_c = window.clone();
                let theme_c = icon_theme.clone();
                let class_c = app_class.clone();

                btn.connect_clicked(move |_| {
                    target_entry_c.set_text(&chosen);
                    let resolved = hypr::resolve_icon_name(&theme_c, &class_c, &chosen);
                    target_preview_c.set_icon_name(Some(&resolved));
                    win_c.close();
                });

                flowbox.append(&btn);
            }
        }
    };

    let initial_query = target_entry.text().trim().to_string();
    let start_q = if !initial_query.is_empty() {
        initial_query
    } else {
        display_class_name(app_class)
    };

    search_entry.set_text(&start_q);
    populate_grid(&start_q);

    let populate_clone = std::rc::Rc::new(populate_grid);
    let pop_ref = populate_clone.clone();
    search_entry.connect_search_changed(move |entry| {
        pop_ref(&entry.text());
    });

    hypr::setup_window_rules();
    window.present();
}

fn setup_drag_and_drop(
    btn: &gtk4::Button,
    app: &PinnedApp,
    hover_popover: &gtk4::Popover,
    cfg: &std::rc::Rc<std::cell::RefCell<config::ConfigFile>>,
) {
    let drag_source = gtk4::DragSource::new();
    drag_source.set_actions(gtk4::gdk::DragAction::MOVE);

    let class_drag = app.class.clone();
    let cfg_drag = cfg.clone();
    drag_source.connect_prepare(move |_, _, _| {
        if cfg_drag.borrow().locked {
            return None;
        }
        Some(gtk4::gdk::ContentProvider::for_value(
            &class_drag.to_value(),
        ))
    });

    let btn_icon = btn.clone();
    let hover_drag = hover_popover.clone();
    drag_source.connect_drag_begin(move |source, _| {
        IS_DRAGGING.store(true, std::sync::atomic::Ordering::Relaxed);
        hover_drag.popdown();
        let paintable = gtk4::WidgetPaintable::new(Some(&btn_icon));
        source.set_icon(Some(&paintable), 0, 0);
    });
    drag_source.connect_drag_end(move |_, _, _| {
        IS_DRAGGING.store(false, std::sync::atomic::Ordering::Relaxed);
    });
    btn.add_controller(drag_source);

    let drop_target = gtk4::DropTarget::new(glib::Type::STRING, gtk4::gdk::DragAction::MOVE);
    let target_class = app.class.clone();
    let cfg_drop = cfg.clone();

    let btn_enter = btn.clone();
    let cfg_enter = cfg.clone();
    drop_target.connect_enter(move |_, _, _| {
        if !cfg_enter.borrow().locked {
            btn_enter.add_css_class("drop-target");
            gtk4::gdk::DragAction::MOVE
        } else {
            gtk4::gdk::DragAction::empty()
        }
    });

    let btn_leave = btn.clone();
    drop_target.connect_leave(move |_| {
        btn_leave.remove_css_class("drop-target");
    });

    let btn_dropped = btn.clone();
    drop_target.connect_drop(move |_, value, _, _| {
        btn_dropped.remove_css_class("drop-target");
        IS_DRAGGING.store(false, std::sync::atomic::Ordering::Relaxed);
        if cfg_drop.borrow().locked {
            return false;
        }
        let Ok(source_class) = value.get::<String>() else {
            return false;
        };
        if same_class(&source_class, &target_class) {
            return false;
        }

        reorder_pinned(&source_class, &target_class);
        EVENT_PENDING.store(true, std::sync::atomic::Ordering::Relaxed);
        true
    });
    btn.add_controller(drop_target);
}

fn reorder_pinned(source_class: &str, target_class: &str) {
    let mut pinned = config::load_pinned();
    let running = hypr::clients();

    // Retrieve PinnedApp entry for source_class
    let source_app = if let Some(idx) = pinned
        .iter()
        .position(|p| same_class(&p.class, source_class))
    {
        pinned.remove(idx)
    } else if let Some(client) = running.iter().find(|c| same_class(&c.class, source_class)) {
        let (icon, cmd) = config::resolve_app_info(&client.class, client.pid);
        PinnedApp {
            class: client.class.clone(),
            label: display_class_name(&client.class),
            icon,
            command: cmd,
        }
    } else {
        return;
    };

    // Determine target_class index in pinned apps
    if let Some(target_idx) = pinned
        .iter()
        .position(|p| same_class(&p.class, target_class))
    {
        pinned.insert(target_idx, source_app);
    } else {
        pinned.push(source_app);
    }

    let _ = config::save_pinned(&pinned);
}

/// Click handler: focus window if already running, otherwise spawn.
fn clone_dock_click(
    _dock: &gtk4::Box,
    app: PinnedApp,
    running: Option<hypr::Client>,
    hover_popover: gtk4::Popover,
) -> impl Fn(&gtk4::Button) + 'static {
    move |_| {
        hover_popover.popdown();
        match &running {
            Some(c) => hypr::focus(c),
            None => hypr::spawn(&app.command),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_class_name() {
        assert_eq!(display_class_name("com.rtosta.zapzap"), "zapzap");
        assert_eq!(display_class_name("org.mozilla.firefox"), "firefox");
        assert_eq!(display_class_name("org.gnome.Nautilus"), "nautilus");
        assert_eq!(display_class_name("dev.zed.Zed"), "zed");
        assert_eq!(display_class_name("Kitty"), "kitty");
        assert_eq!(display_class_name("com.rtosta.zapzap."), "zapzap");
        assert_eq!(display_class_name(""), "");
    }
}
