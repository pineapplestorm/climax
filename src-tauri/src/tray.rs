// System tray icon, menu, and live timer in tooltip.
//
// Note: Tauri 2 exposes set_menu but not menu read-back on TrayIcon, so we don't
// mutate menu labels at runtime. Click handlers infer current state from the DB.
// The live timer / status is reflected in the TOOLTIP, which is much more
// visible to the user anyway.

use std::time::Duration;

use anyhow::Result;
use tauri::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    tray::{TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, Runtime,
};

use crate::remote_session;
use crate::state::AppState;

const ID_START_STOP: &str = "climax_toggle";
const ID_PAUSE: &str = "climax_pause";
const ID_OPEN_TRACKER: &str = "climax_open_tracker";
const ID_OPEN_DASHBOARD: &str = "climax_open_dashboard";
const ID_QUIT: &str = "climax_quit";

pub fn build<R: Runtime>(app: &AppHandle<R>) -> Result<TrayIcon<R>> {
    let toggle = MenuItem::with_id(app, ID_START_STOP, "Start / stop session", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, ID_PAUSE, "Pause / resume", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let open_tracker = MenuItem::with_id(app, ID_OPEN_TRACKER, "Open tracker", true, None::<&str>)?;
    let open_dashboard = MenuItem::with_id(app, ID_OPEN_DASHBOARD, "Open dashboard", true, None::<&str>)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, ID_QUIT, "Quit Climax", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&toggle, &pause, &sep1, &open_tracker, &open_dashboard, &sep2, &quit])?;

    let builder = TrayIconBuilder::with_id("climax-tray")
        .tooltip("Climax - idle")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(handle_menu)
        .on_tray_icon_event(handle_icon);

    // macOS menu-bar items want an alpha-only TEMPLATE image that the system
    // tints itself; a coloured bitmap reads as a sticker beside native items.
    // Start on the not-tracking icon - there is never a session at boot.
    #[cfg(target_os = "macos")]
    let builder = builder
        .icon(tauri::image::Image::from_bytes(include_bytes!(
            "../icons/tray-mac-idle@2x.png"
        ))?)
        .icon_as_template(true);
    #[cfg(not(target_os = "macos"))]
    let builder = builder.icon(app.default_window_icon().unwrap().clone());

    Ok(builder.build(app)?)
}

pub fn spawn_timer_updater<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        // Tray icons, loaded once. All embedded in the binary — none reference
        // any external file at runtime. This swap is the SYSTEM-TRAY icon ONLY
        // — the taskbar button, window icon, and app/shortcut icon are all
        // untouched.
        //
        // Windows/Linux: THREE colour variants.
        //   active   = the coloured mark (the app's default window icon, which
        //              Tauri bakes in from src-tauri/icons/icon.png)
        //   disabled = grey "not tracking" variant   (icons/tray-disabled.png)
        //   paused   = the paused variant            (icons/tray-paused.png)
        #[cfg(not(target_os = "macos"))]
        let (active_icon, disabled_icon, paused_icon) = (
            app.default_window_icon().cloned(),
            tauri::image::Image::from_bytes(include_bytes!("../icons/tray-disabled.png")).ok(),
            tauri::image::Image::from_bytes(include_bytes!("../icons/tray-paused.png")).ok(),
        );

        // macOS: TWO alpha-only template images. A template is tinted by the
        // system (black on a light menu bar, white on a dark one, inverted
        // while the menu is open), so colour cannot carry state at all there.
        // Decided with the user: SOLID = a live session exists, running OR
        // paused; GREYED = no session. Paused deliberately looks identical to
        // running, because greyed means "not tracking" and nothing else.
        //
        // @2x is the right source: tray-icon forces the image to 18pt tall
        // whatever you hand it, so 44px gives crisp backing on a Retina display
        // (which needs 36px) while a 22px file would be upscaled and blur.
        #[cfg(target_os = "macos")]
        let (mac_live_icon, mac_idle_icon) = (
            tauri::image::Image::from_bytes(include_bytes!("../icons/tray-mac-active@2x.png")).ok(),
            tauri::image::Image::from_bytes(include_bytes!("../icons/tray-mac-idle@2x.png")).ok(),
        );

        // Only call set_icon on a state CHANGE, not every tick.
        // 0 = idle (no session), 1 = active, 2 = paused. None = not yet applied.
        let mut last_state: Option<u8> = None;
        // Same idea for the macOS menu bar title beside the icon.
        #[cfg(target_os = "macos")]
        let mut last_menubar: Option<String> = None;

        // Poll cadence. In-process: 1s - a free local DB read (unchanged). In
        // client mode each tick is an HTTP /rpc round-trip, so settle to a slower
        // cadence after the first tick to avoid hammering a remote (NAS/LAN)
        // server every second purely to refresh a tray icon that only changes on
        // start/stop/pause (the tooltip is hover-only, so a few seconds' lag is
        // fine). Set from `state` after the first fetch, so the default 1s path is
        // byte-identical (interval_secs stays 1 forever when not a client).
        let mut interval_secs: u64 = 1;

        // The word the tooltip uses for an O follows the Wording setting. It
        // changes about once in an install's life, so re-read it every tenth
        // tick instead of every tick (in client mode each read is a round-trip).
        let mut says_orgasm = false;
        let mut ticks: u32 = 0;

        loop {
            tokio::time::sleep(Duration::from_secs(interval_secs)).await;
            let Some(state) = app.try_state::<AppState>() else { continue };
            interval_secs = if state.client_server_url.is_some() { 3 } else { 1 };
            if ticks.is_multiple_of(10) {
                says_orgasm = remote_session::says_orgasm(&state).await;
            }
            ticks = ticks.wrapping_add(1);

            // active() returns Some for both `active` AND `paused` sessions
            // (paused rows still have ended_at = NULL). A paused session gets
            // the paused icon; a running one the coloured icon; no session at
            // all the grey icon. In client mode this reflects the SERVER's
            // session (an unreachable server reads as idle, the safe fallback).
            let active = remote_session::active(&state).await;

            let (icon_state, tooltip) = match &active {
                Some(s) => {
                    let dur = format_duration(s.effective_duration_ms);
                    let paused = s.status == "paused";
                    let status = if paused { "paused" } else { "running" };
                    let scene_plural = if s.scene_count == 1 { "scene" } else { "scenes" };
                    let cumshot_plural = match (says_orgasm, s.o_count == 1) {
                        (false, true) => "cumshot",
                        (false, false) => "cumshots",
                        (true, true) => "orgasm",
                        (true, false) => "orgasms",
                    };
                    let tooltip = format!(
                        "Climax \u{2022} {} \u{2022} {} \u{2022} {} {} \u{2022} {} {}",
                        dur, status, s.scene_count, scene_plural, s.o_count, cumshot_plural
                    );
                    (if paused { 2u8 } else { 1u8 }, tooltip)
                }
                None => (0u8, "Climax - idle (click to start a session)".to_string()),
            };

            // No session: icon only, no text - the menu bar stays as narrow as
            // every other item until there is something to report.
            //
            // GOTCHA: tray-icon's macOS `set_title` is `if let Some(title)`, so
            // passing None does NOTHING - it cannot clear a title already on
            // screen, and the stale figure just sits there after the session
            // ends. Always pass Some; empty string is what actually clears it.
            #[cfg(target_os = "macos")]
            let menubar: Option<String> = Some(match &active {
                Some(s) => format_menubar(s.effective_duration_ms),
                None => String::new(),
            });

            if let Some(tray) = app.tray_by_id("climax-tray") {
                // macOS menu bar items have NO hover tooltip, so this is a no-op
                // there; the title below carries the live figure instead.
                let _ = tray.set_tooltip(Some(tooltip));

                // Only write on CHANGE. The title is minute-resolution, so this
                // is ~once a minute rather than every poll.
                #[cfg(target_os = "macos")]
                if last_menubar != menubar {
                    let _ = tray.set_title(menubar.clone());
                    last_menubar = menubar;
                }

                if last_state != Some(icon_state) {
                    #[cfg(not(target_os = "macos"))]
                    {
                        let next = match icon_state {
                            2 => paused_icon.clone(),
                            1 => active_icon.clone(),
                            _ => disabled_icon.clone(),
                        };
                        if let Some(icon) = next {
                            let _ = tray.set_icon(Some(icon));
                        }
                    }
                    #[cfg(target_os = "macos")]
                    {
                        // 1 = running, 2 = paused: both are a live session, so
                        // both are solid. Only 0 (no session) greys out.
                        let next = if icon_state == 0 {
                            mac_idle_icon.clone()
                        } else {
                            mac_live_icon.clone()
                        };
                        if let Some(icon) = next {
                            // Template status lives on the NSImage, so a plain
                            // set_icon would DROP it and the glyph would render
                            // as an opaque black blob. Setting both together is
                            // also the documented way to dodge the flicker that
                            // set_icon followed by set_icon_as_template causes.
                            let _ = tray.set_icon_with_as_template(Some(icon), true);
                        }
                    }
                    last_state = Some(icon_state);
                }
            }
        }
    });
}

fn handle_menu<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let id = event.id.as_ref().to_string();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(state) = app.try_state::<AppState>() else { return };
        match id.as_str() {
            ID_START_STOP => {
                // In client mode these drive the SERVER's session over /rpc. Use
                // the error-surfacing read so a transient unreachable server (which
                // the lenient active() reads as None) can't trick the toggle into
                // START-ing - on the server that ENDs the running session and
                // replaces it (start isn't idempotent). On an uncertain read, do
                // nothing rather than risk killing the live session.
                match remote_session::active_checked(&state).await {
                    Ok(Some(_)) => {
                        let _ = remote_session::stop(&state).await;
                    }
                    Ok(None) => {
                        let _ = remote_session::start(&state).await;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "tray toggle: couldn't read session state, doing nothing: {:#}",
                            e
                        );
                    }
                }
            }
            ID_PAUSE => {
                if let Some(s) = remote_session::active(&state).await {
                    if s.status == "paused" {
                        let _ = remote_session::resume(&state).await;
                    } else if s.status == "active" {
                        let _ = remote_session::pause(&state).await;
                    }
                }
            }
            ID_OPEN_TRACKER => crate::commands::show_solo(&app, "tracker"),
            ID_OPEN_DASHBOARD => crate::commands::show_solo(&app, "dashboard"),
            ID_QUIT => {
                // Quitting with a session running is intercepted: show the
                // tracker's wrap-up-or-go-back prompt instead of exiting. The
                // tracker stops the session (wrap-up) then calls confirm_quit,
                // or cancels via cancel_quit. With no session, exit immediately.
                // In client mode this asks the SERVER - the prompt's "Close
                // without ending" then leaves the session running on the server.
                let has_session = remote_session::active_id(&state).await.is_some();
                if has_session {
                    *state.pending_quit.write().await = true;
                    if let Some(dash) = app.get_webview_window("dashboard") {
                        let _ = dash.hide();
                    }
                    if let Some(win) = app.get_webview_window("tracker") {
                        let _ = win.set_always_on_top(true);
                        let _ = win.show();
                        let _ = win.unminimize();
                        let _ = win.set_focus();
                    }
                    let _ = app.emit("quit_requested", ());
                } else {
                    app.exit(0);
                }
            }
            _ => {}
        }
    });
}

fn handle_icon<R: Runtime>(tray: &TrayIcon<R>, event: TrayIconEvent) {
    // Single left click: open the tracker (compact widget - the daily-use surface).
    // show_solo also hides the dashboard if it's currently visible.
    if let TrayIconEvent::Click {
        button: tauri::tray::MouseButton::Left,
        button_state: tauri::tray::MouseButtonState::Up,
        ..
    } = event
    {
        crate::commands::show_solo(tray.app_handle(), "tracker");
    }
}

/// Compact duration for the macOS MENU BAR title, beside the icon.
///
/// Deliberately not `format_duration`: `01:23:45` is eight characters of
/// permanently-occupied menu bar, and the seconds redraw every tick, so the
/// text jitters and shoves its neighbours about. This is at most five
/// (`12h05`), and only changes once a minute - the same restraint the system
/// stats apps show. Under an hour it drops to `7m`.
#[cfg(target_os = "macos")]
fn format_menubar(ms: i64) -> String {
    let total_mins = (ms / 60_000).max(0);
    let h = total_mins / 60;
    let m = total_mins % 60;
    if h > 0 {
        format!("{}h{:02}", h, m)
    } else {
        format!("{}m", m)
    }
}

fn format_duration(ms: i64) -> String {
    let total_secs = (ms / 1000).max(0);
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    if h > 0 {
        format!("{:02}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}
