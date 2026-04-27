#[cfg(not(target_os = "macos"))]
use enigo::{Button, Coordinate, Direction, Enigo, Mouse, Settings};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use xcap::Monitor;

// ---- Debug log file ----
//
// When the user flips the debug-log toggle, JS calls `debug_log_start()`,
// which opens a timestamped file under ~/Desktop/fisch-macro-debug/ and
// records the start instant. Every subsequent `debug_log_append()` writes
// "[+elapsed_s] <line>\n" and flushes, so the file is up-to-date even if
// the user kills the app abruptly.
struct DebugLog {
    file: std::fs::File,
    started_at: Instant,
}
fn debug_log_lock() -> &'static Mutex<Option<DebugLog>> {
    static SLOT: OnceLock<Mutex<Option<DebugLog>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

// ---- Background spam loops (Enter, M1 rapid-click) ----
//
// Both spam loops live in dedicated OS threads instead of JS setIntervals.
// JS setInterval has ~10ms scheduling jitter and adds Tauri IPC overhead per
// tick (typically ~3-5ms), which made our SHAKE Enter-spam noticeably slower
// than a human pressing the key. A Rust thread sleeping on the OS scheduler
// hits its target interval cleanly and posts the event with no extra hops.
//
// Atomics control the threads from the main thread:
//   *_RUNNING  — flip false to stop the loop on its next iteration
//   *_INTERVAL — live-tunable in case JS wants to change cadence mid-run
static ENTER_SPAM_RUNNING: AtomicBool = AtomicBool::new(false);
static ENTER_SPAM_INTERVAL_MS: AtomicU64 = AtomicU64::new(50);
static M1_CLICK_RUNNING: AtomicBool = AtomicBool::new(false);
static M1_CLICK_INTERVAL_MS: AtomicU64 = AtomicU64::new(25);
// PWM-style M1 control. Decouples M1 actuation from the JS detection tick:
// JS sets a duty cycle each tick (0-100%) and a tight Rust thread executes
// that duty cycle at fine granularity. Solves the "binary HOLD/RELEASE
// swings cause overshoot" problem when the game has 100-150ms input lag.
//   M1_PWM_RUNNING — flip false to stop the loop
//   M1_PWM_DUTY    — 0-100, fraction of cycle M1 is held down
//   M1_PWM_CYCLE_MS — total cycle length (recommended 30-50ms)
static M1_PWM_RUNNING: AtomicBool = AtomicBool::new(false);
static M1_PWM_DUTY: AtomicU64 = AtomicU64::new(50);
static M1_PWM_CYCLE_MS: AtomicU64 = AtomicU64::new(40);

// Previous tick's fish_bar pixel buffer, used for temporal motion detection.
// Tier-2 fish tracking: instead of color matching (which keeps locking onto
// fixed UI elements like end-cap markers), we diff this frame against the
// previous frame and find columns with significant pixel changes. Static UI
// = no change. Bar at rest = no change. Fish moving = lots of change. Largest
// motion cluster outside the player bar's columns ≈ fish position.
fn prev_fish_frame() -> &'static Mutex<Option<(u32, u32, Vec<u8>)>> {
    static LOCK: OnceLock<Mutex<Option<(u32, u32, Vec<u8>)>>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(None))
}

/// Baseline snapshot of the shake region (RGB), captured when no SHAKE button
/// is visible. Detection then looks for pixels that DIFFER from baseline —
/// scenery is static, the button popping in is a huge change.
///
/// `excludes_at_capture` records rects that were on screen at baseline time
/// (e.g. the macro window). At detection time these are still skipped even
/// if they've moved, so the move itself doesn't poison the diff.
struct ShakeBaseline {
    width: u32,
    height: u32,
    rgb: Vec<u8>, // 3 bytes per pixel, row-major
    excludes_at_capture: Vec<(i32, i32, i32, i32)>,
}

static BASELINE: OnceLock<Mutex<Option<ShakeBaseline>>> = OnceLock::new();
fn baseline_lock() -> &'static Mutex<Option<ShakeBaseline>> {
    BASELINE.get_or_init(|| Mutex::new(None))
}

/// Template image of the SHAKE button captured by the user. Stored as packed
/// RGB. Detection slides this template across the shake region every tick
/// and reports the position with the lowest sum-of-absolute-differences.
struct ShakeTemplate {
    width: u32,
    height: u32,
    rgb: Vec<u8>, // 3 bytes per pixel, row-major
}

static TEMPLATE: OnceLock<Mutex<Option<ShakeTemplate>>> = OnceLock::new();
fn template_lock() -> &'static Mutex<Option<ShakeTemplate>> {
    TEMPLATE.get_or_init(|| Mutex::new(None))
}

// Target-window setting. When non-empty, capture functions try window-
// targeted capture (via xcap's Window::capture_image, which on macOS uses
// CGWindowListCreateImage under the hood) before falling back to whole-
// display capture. This is what makes the macro work when Roblox is in
// macOS native fullscreen on a different Space — display capture from the
// macro's own Space sees nothing, but window capture follows the window.
fn target_window_lock() -> &'static std::sync::Mutex<String> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<String>> =
        std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(String::new()))
}

fn get_target_window() -> String {
    target_window_lock()
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

/// JS calls this when the user changes the target-window setting.
#[tauri::command]
fn set_target_window(name: String) -> Result<(), String> {
    if let Ok(mut g) = target_window_lock().lock() {
        *g = name;
    }
    Ok(())
}

#[derive(Serialize)]
struct WindowEntry {
    app_name: String,
    title: String,
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    minimized: bool,
}

/// Enumerate all windows xcap can see. Used by the JS UI to show the
/// user what windows exist so they can pick the right target name. Helps
/// debug "Target window 'Roblox' not found" — if the fullscreen game's
/// xcap-reported name is different, the user can see it here and update.
#[tauri::command]
fn list_windows() -> Result<Vec<WindowEntry>, String> {
    use xcap::Window;
    let windows = Window::all().map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(windows.len());
    for w in &windows {
        out.push(WindowEntry {
            app_name: w.app_name().unwrap_or_default(),
            title: w.title().unwrap_or_default(),
            width: w.width().unwrap_or(0),
            height: w.height().unwrap_or(0),
            x: w.x().unwrap_or(0),
            y: w.y().unwrap_or(0),
            minimized: w.is_minimized().unwrap_or(false),
        });
    }
    Ok(out)
}

#[cfg(target_os = "macos")]
mod cg_capture {
    use std::ffi::c_void;
    use xcap::{Monitor, Window};

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CGPoint {
        pub x: f64,
        pub y: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CGSize {
        pub width: f64,
        pub height: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CGRect {
        pub origin: CGPoint,
        pub size: CGSize,
    }

    pub type CGImageRef = *mut c_void;
    pub type CGDataProviderRef = *mut c_void;
    pub type CFDataRef = *mut c_void;

    #[link(name = "CoreGraphics", kind = "framework")]
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub fn CGMainDisplayID() -> u32;
        pub fn CGDisplayCreateImageForRect(display: u32, rect: CGRect) -> CGImageRef;
        pub fn CGImageGetWidth(image: CGImageRef) -> usize;
        pub fn CGImageGetHeight(image: CGImageRef) -> usize;
        pub fn CGImageGetBytesPerRow(image: CGImageRef) -> usize;
        pub fn CGImageGetDataProvider(image: CGImageRef) -> CGDataProviderRef;
        pub fn CGImageRelease(image: CGImageRef);
        pub fn CGDataProviderCopyData(provider: CGDataProviderRef) -> CFDataRef;
        pub fn CFDataGetBytePtr(data: CFDataRef) -> *const u8;
        pub fn CFDataGetLength(data: CFDataRef) -> isize;
        pub fn CFRelease(obj: *mut c_void);
    }

    /// A view into the screen rect requested by `capture_logical_rect`.
    /// Internally we always capture the FULL display (because
    /// `CGDisplayCreateImageForRect` with a non-zero origin returns the wrong
    /// pixel region on macOS 14+ in HiDPI-scaled display modes — the size is
    /// honored but the origin is silently ignored or remapped). `pixel_at`
    /// translates rect-relative offsets back into the full-screen buffer.
    ///
    /// `width`/`height` are physical pixels of the requested rect (so on
    /// Retina, w*2 × h*2 even though caller asked for w × h logical).
    pub struct PartialCapture {
        pub width: u32,
        #[allow(dead_code)]
        pub height: u32,
        bytes_per_row: usize,
        data: Vec<u8>,
        offset_x_phys: u32,
        offset_y_phys: u32,
    }

    impl PartialCapture {
        /// Read a pixel at PHYSICAL offset (px, py) within the requested rect.
        /// Returns [R, G, B, A]. Out-of-bounds reads return [0, 0, 0, 0]
        /// instead of panicking — happens at the right/bottom edge when the
        /// requested rect is exactly flush with the captured image and an
        /// off-by-one in dpr rounding tips us past the last byte.
        #[inline]
        pub fn pixel_at(&self, px: u32, py: u32) -> [u8; 4] {
            let abs_x = self.offset_x_phys + px;
            let abs_y = self.offset_y_phys + py;
            let idx = (abs_y as usize) * self.bytes_per_row + (abs_x as usize) * 4;
            if idx + 3 >= self.data.len() {
                return [0, 0, 0, 0];
            }
            // macOS stores pixels as BGRA; convert to RGBA on read.
            let b = self.data[idx];
            let g = self.data[idx + 1];
            let r = self.data[idx + 2];
            let a = self.data[idx + 3];
            [r, g, b, a]
        }
    }

    /// Window-targeted capture: find the window whose app-name or title
    /// case-insensitively contains `name`, capture its full image (which
    /// on macOS uses CGWindowListCreateImage under the hood — works
    /// across Spaces, even when the window is in native fullscreen on a
    /// different Space than the macro window), then crop to the screen-
    /// absolute rect (sx, sy, sw, sh). Returns None if the window can't
    /// be found, so callers can fall back to display capture.
    ///
    /// xcap's RgbaImage is RGBA in memory; PartialCapture's pixel_at
    /// expects BGRA (matching what CGDisplayCreateImageForRect returns)
    /// so we swap channels at copy time. The cost is ~one R/B swap pass
    /// over the captured pixels — still well under our tick budget.
    pub fn capture_window_rect_by_name(
        name: &str,
        sx: i32,
        sy: i32,
        sw: u32,
        sh: u32,
    ) -> Option<PartialCapture> {
        if name.is_empty() || sw == 0 || sh == 0 {
            return None;
        }
        let needle = name.to_lowercase();

        let windows = Window::all().ok()?;
        // Match on app_name OR title (case-insensitive substring), prefer
        // the largest matching window so we pick the actual game window
        // rather than e.g. a small "About" dialog.
        let mut best: Option<(u32, &Window)> = None;
        for w in &windows {
            let app = w.app_name().unwrap_or_default().to_lowercase();
            let title = w.title().unwrap_or_default().to_lowercase();
            if !app.contains(&needle) && !title.contains(&needle) {
                continue;
            }
            // Skip minimized windows — capture would be empty.
            if w.is_minimized().unwrap_or(false) {
                continue;
            }
            let area = w.width().unwrap_or(0).saturating_mul(w.height().unwrap_or(0));
            if best.as_ref().map_or(true, |(a, _)| area > *a) {
                best = Some((area, w));
            }
        }
        let (_, win) = best?;

        let win_x = win.x().ok()?;
        let win_y = win.y().ok()?;
        let win_w_logical = win.width().ok()?;
        if win_w_logical == 0 {
            return None;
        }

        let img = win.capture_image().ok()?;
        let img_w = img.width();
        let img_h = img.height();

        // DPR = physical / logical. Round to nearest int.
        let dpr_f = img_w as f64 / win_w_logical as f64;
        let dpr = dpr_f.round().max(1.0) as u32;

        // Swap RGBA → BGRA so existing PartialCapture::pixel_at works.
        let rgba: Vec<u8> = img.into_raw();
        let mut bgra: Vec<u8> = Vec::with_capacity(rgba.len());
        for chunk in rgba.chunks_exact(4) {
            bgra.push(chunk[2]);
            bgra.push(chunk[1]);
            bgra.push(chunk[0]);
            bgra.push(chunk[3]);
        }

        // Convert screen-absolute (sx, sy) into window-relative logical
        // pixels, then to physical via dpr.
        let rel_x_logical = (sx - win_x).max(0) as u32;
        let rel_y_logical = (sy - win_y).max(0) as u32;
        let off_x = rel_x_logical.saturating_mul(dpr).min(img_w);
        let off_y = rel_y_logical.saturating_mul(dpr).min(img_h);
        let req_w = sw.saturating_mul(dpr).min(img_w.saturating_sub(off_x));
        let req_h = sh.saturating_mul(dpr).min(img_h.saturating_sub(off_y));

        Some(PartialCapture {
            width: req_w,
            height: req_h,
            bytes_per_row: (img_w as usize) * 4,
            data: bgra,
            offset_x_phys: off_x,
            offset_y_phys: off_y,
        })
    }

    /// Capture the rectangle (x, y, w, h) in LOGICAL pixels. The returned
    /// view is in PHYSICAL pixels (Retina = 2× the logical size).
    ///
    /// If a target-window name is set globally (via set_target_window),
    /// try window capture first — that's what makes the macro work with
    /// macOS native fullscreen on a different Space. Falls back to whole-
    /// display capture if the target window isn't found / isn't
    /// configured.
    ///
    /// IMPLEMENTATION NOTE for the display-capture path: we always capture
    /// the full display via `CGDisplayCreateImageForRect(display, full_bounds)`
    /// and crop in-process. Calling CG with a non-zero origin returned wrong
    /// pixels on this user's macOS 14+ HiDPI-scaled setup; full-screen
    /// capture is reliable.
    pub fn capture_logical_rect(x: i32, y: i32, w: u32, h: u32) -> Result<PartialCapture, String> {
        // Try window-targeted capture first.
        let target = super::get_target_window();
        if !target.is_empty() {
            if let Some(cap) = capture_window_rect_by_name(&target, x, y, w, h) {
                return Ok(cap);
            }
        }
        // Fall through to display capture.
        capture_logical_rect_display(x, y, w, h)
    }

    fn capture_logical_rect_display(x: i32, y: i32, w: u32, h: u32) -> Result<PartialCapture, String> {
        if w == 0 || h == 0 {
            return Err("Empty capture rect".to_string());
        }

        // Get logical display size from xcap, NOT CGDisplayBounds. On macOS
        // HiDPI-scaled modes, CGDisplayBounds reports the native panel
        // dimensions (e.g. 3456×2234 on a 16" MBP) instead of the points
        // the rest of the system uses (e.g. 1710×1112). xcap's Monitor
        // reports the logical-points value consistently.
        let monitors = Monitor::all().map_err(|e| e.to_string())?;
        let monitor = monitors.first().ok_or("No monitor found")?;
        let mw_logical = monitor.width().map_err(|e| e.to_string())?;
        let mh_logical = monitor.height().map_err(|e| e.to_string())?;
        if mw_logical == 0 || mh_logical == 0 {
            return Err("Monitor reports zero size".to_string());
        }

        unsafe {
            let display = CGMainDisplayID();
            let rect = CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize {
                    width: mw_logical as f64,
                    height: mh_logical as f64,
                },
            };
            let img = CGDisplayCreateImageForRect(display, rect);
            if img.is_null() {
                return Err("CGDisplayCreateImageForRect returned null".to_string());
            }

            let img_w = CGImageGetWidth(img) as u32;
            let img_h = CGImageGetHeight(img) as u32;
            let bytes_per_row = CGImageGetBytesPerRow(img);

            let provider = CGImageGetDataProvider(img);
            let data = CGDataProviderCopyData(provider);
            let len = CFDataGetLength(data) as usize;
            let ptr = CFDataGetBytePtr(data);
            let mut buf = Vec::with_capacity(len);
            buf.extend_from_slice(std::slice::from_raw_parts(ptr, len));

            CFRelease(data);
            CGImageRelease(img);

            // Compute dpr from xcap-reported logical width vs CG-reported
            // physical pixels. Round to nearest int to handle the slight
            // mismatch in HiDPI scaled modes (e.g. 3420/1710 = 2.0 exactly,
            // but on some scaled modes you get 2.02-ish).
            let dpr_f = img_w as f64 / mw_logical as f64;
            let dpr = dpr_f.round().max(1.0) as u32;

            // Convert requested logical rect → physical offsets in the full
            // buffer. Clamp so reads can't run past the captured image.
            let off_x = (x.max(0) as u32).saturating_mul(dpr).min(img_w);
            let off_y = (y.max(0) as u32).saturating_mul(dpr).min(img_h);
            let req_w = w.saturating_mul(dpr).min(img_w.saturating_sub(off_x));
            let req_h = h.saturating_mul(dpr).min(img_h.saturating_sub(off_y));

            Ok(PartialCapture {
                width: req_w,
                height: req_h,
                bytes_per_row,
                data: buf,
                offset_x_phys: off_x,
                offset_y_phys: off_y,
            })
        }
    }
}

#[cfg(target_os = "macos")]
mod cg_mouse {
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CGPoint {
        pub x: f64,
        pub y: f64,
    }

    pub type CGEventRef = *mut c_void;
    pub type CGEventSourceRef = *mut c_void;

    // CGEventTapLocation
    const K_CG_HID_EVENT_TAP: u32 = 0;
    // CGEventType
    const K_CG_EVENT_LEFT_MOUSE_DOWN: u32 = 1;
    const K_CG_EVENT_LEFT_MOUSE_UP: u32 = 2;
    const K_CG_EVENT_MOUSE_MOVED: u32 = 5;
    // CGMouseButton
    const K_CG_MOUSE_BUTTON_LEFT: u32 = 0;
    // CGEventField — kCGMouseEventClickState
    const K_CG_MOUSE_EVENT_CLICK_STATE: u32 = 1;
    // macOS virtual keycodes (HIToolbox / Events.h)
    const KEY_RETURN: u16 = 36;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        pub fn CGWarpMouseCursorPosition(point: CGPoint) -> i32;
        pub fn CGEventCreate(source: CGEventSourceRef) -> CGEventRef;
        pub fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
        pub fn CGEventCreateMouseEvent(
            source: CGEventSourceRef,
            mouse_type: u32,
            position: CGPoint,
            button: u32,
        ) -> CGEventRef;
        pub fn CGEventCreateKeyboardEvent(
            source: CGEventSourceRef,
            virtual_key: u16,
            key_down: bool,
        ) -> CGEventRef;
        pub fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
        pub fn CGEventPost(tap: u32, event: CGEventRef);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub fn CFRelease(obj: *mut c_void);
    }

    /// Hard-warp cursor to (x, y) in logical points and also send a
    /// synthetic MouseMoved event at the same location. The warp moves the
    /// visible cursor immediately, but it does NOT generate a MouseMoved
    /// event — so apps that track cursor position via the event stream
    /// (Roblox is one) keep using a stale position for hit-testing
    /// subsequent mouse events. Posting the MouseMoved here keeps those
    /// apps' internal cursor model in sync with the actual on-screen cursor.
    pub fn warp(x: f64, y: f64) -> Result<(), String> {
        let r = unsafe { CGWarpMouseCursorPosition(CGPoint { x, y }) };
        if r != 0 {
            return Err(format!("CGWarpMouseCursorPosition failed: {}", r));
        }
        post_button(K_CG_EVENT_MOUSE_MOVED, x, y)?;
        Ok(())
    }

    /// Read the OS's current cursor position. We use this for button events
    /// because `enigo` on macOS posts events at its OWN internally-tracked
    /// position (which we never update — we use CGWarpMouseCursorPosition for
    /// movement). Querying the live cursor lets us post button events at the
    /// actual on-screen cursor location.
    fn current_pos() -> CGPoint {
        unsafe {
            let evt = CGEventCreate(std::ptr::null_mut());
            if evt.is_null() {
                return CGPoint { x: 0.0, y: 0.0 };
            }
            let pos = CGEventGetLocation(evt);
            CFRelease(evt);
            pos
        }
    }

    fn post_button(event_type: u32, x: f64, y: f64) -> Result<(), String> {
        unsafe {
            let event = CGEventCreateMouseEvent(
                std::ptr::null_mut(),
                event_type,
                CGPoint { x, y },
                K_CG_MOUSE_BUTTON_LEFT,
            );
            if event.is_null() {
                return Err("CGEventCreateMouseEvent returned null".to_string());
            }
            // Click state defaults to 0 ("not a real click"). Roblox (and many
            // other apps) ignore mouse-down/up events with click state 0 — the
            // event is delivered but treated as phantom motion, never producing
            // a real click. Setting it to 1 makes this a genuine single click.
            CGEventSetIntegerValueField(event, K_CG_MOUSE_EVENT_CLICK_STATE, 1);
            CGEventPost(K_CG_HID_EVENT_TAP, event);
            CFRelease(event);
        }
        Ok(())
    }

    /// Press left mouse button at the current cursor location.
    pub fn left_button_down() -> Result<(), String> {
        let pos = current_pos();
        post_button(K_CG_EVENT_LEFT_MOUSE_DOWN, pos.x, pos.y)
    }

    /// Release left mouse button at the current cursor location.
    pub fn left_button_up() -> Result<(), String> {
        let pos = current_pos();
        post_button(K_CG_EVENT_LEFT_MOUSE_UP, pos.x, pos.y)
    }

    /// Press left mouse at an explicit position. We send a MouseMoved event
    /// first because Roblox (and some other apps) hit-test mouse-button
    /// events against the cursor's last-tracked position, not against the
    /// event's explicit `position` field. CGWarpMouseCursorPosition doesn't
    /// generate a MouseMoved event, so without this prelude the click lands
    /// at the cursor's PREVIOUS position from the app's point of view —
    /// even though the cursor visually appears at the new location.
    pub fn left_button_down_at(x: f64, y: f64) -> Result<(), String> {
        post_button(K_CG_EVENT_MOUSE_MOVED, x, y)?;
        post_button(K_CG_EVENT_LEFT_MOUSE_DOWN, x, y)
    }

    /// Release left mouse at an explicit position.
    pub fn left_button_up_at(x: f64, y: f64) -> Result<(), String> {
        post_button(K_CG_EVENT_LEFT_MOUSE_UP, x, y)
    }

    /// Tap a key by virtual keycode — emits keyDown, very brief sleep, keyUp.
    /// Used for Enter-spam during the reel loop, which is how Roblox Fisch's
    /// SHAKE prompt is dismissed without needing image detection. The 5ms
    /// gap is short enough that the JS layer can drive 100+ presses/sec
    /// when the user wants a faster spam rate.
    fn tap_key(key: u16) -> Result<(), String> {
        unsafe {
            let down = CGEventCreateKeyboardEvent(std::ptr::null_mut(), key, true);
            if down.is_null() {
                return Err("CGEventCreateKeyboardEvent (down) returned null".to_string());
            }
            CGEventPost(K_CG_HID_EVENT_TAP, down);
            CFRelease(down);
            std::thread::sleep(std::time::Duration::from_millis(5));
            let up = CGEventCreateKeyboardEvent(std::ptr::null_mut(), key, false);
            if up.is_null() {
                return Err("CGEventCreateKeyboardEvent (up) returned null".to_string());
            }
            CGEventPost(K_CG_HID_EVENT_TAP, up);
            CFRelease(up);
        }
        Ok(())
    }

    /// Tap the Return / Enter key.
    pub fn press_return() -> Result<(), String> {
        tap_key(KEY_RETURN)
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct Region {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

// User-tunable fish-bar detection parameters. Mirrors Hydra's "Color"
// track style: per-element RGB hex with per-channel tolerance + detection
// thresholds. Sent on every tick from JS so the user can adjust live
// without rebuilding the Rust side.
#[derive(Deserialize, Clone)]
#[allow(dead_code)] // arrow/arrow_tol reserved for upcoming arrow detection
struct FishColorParams {
    target_line: [u8; 3],
    arrow: [u8; 3],
    left_bar: [u8; 3],
    right_bar: [u8; 3],
    target_tol: u8,
    arrow_tol: u8,
    left_tol: u8,
    right_tol: u8,
    // 0-100 — Hydra's "White % Required" (per-column pixel match rate
    // for the player bar).
    white_pct: u8,
    // 0-100 — Hydra's "Min Line Density" (per-column pixel match rate
    // for the target line).
    min_line_density: u8,
    // Pixels — small inset from each region edge to skip frame artifacts.
    edge_touch: u32,
    // Pixels — bridge gaps of this size when joining player-bar runs
    // (Hydra's "Merge Distance").
    merge_distance: u32,
    // Pixels — minimum width for a player-bar run to be considered valid
    // (Hydra's "Min Line Count").
    min_line_count: u32,
}

impl Default for FishColorParams {
    fn default() -> Self {
        // Hydra's defaults straight from the user's screenshots.
        FishColorParams {
            target_line: [0x43, 0x4b, 0x5b],
            arrow: [0x84, 0x85, 0x87],
            left_bar: [0xf1, 0xf1, 0xf1],
            right_bar: [0xff, 0xff, 0xff],
            target_tol: 2,
            arrow_tol: 0,
            left_tol: 3,
            right_tol: 3,
            white_pct: 80,
            min_line_density: 80,
            edge_touch: 1,
            merge_distance: 2,
            min_line_count: 4,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct RegionsConfig {
    screen_width: u32,
    screen_height: u32,
    shake: Option<Region>,
    fish_bar: Option<Region>,
    #[serde(default)]
    shake_template: Option<Region>,
}

fn config_path() -> Result<PathBuf, String> {
    let mut dir = dirs::config_dir().ok_or("No config dir")?;
    dir.push("fisch-macro");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    dir.push("regions.json");
    Ok(dir)
}

/// Path to the persisted SHAKE template binary. Format: 4-byte LE width,
/// 4-byte LE height, then width*height*3 bytes of packed RGB.
fn template_path() -> Result<PathBuf, String> {
    let mut dir = dirs::config_dir().ok_or("No config dir")?;
    dir.push("fisch-macro");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    dir.push("shake_template.bin");
    Ok(dir)
}

fn save_template_to_disk(t: &ShakeTemplate) -> Result<(), String> {
    let path = template_path()?;
    let mut bytes = Vec::with_capacity(8 + t.rgb.len());
    bytes.extend_from_slice(&t.width.to_le_bytes());
    bytes.extend_from_slice(&t.height.to_le_bytes());
    bytes.extend_from_slice(&t.rgb);
    fs::write(&path, bytes).map_err(|e| e.to_string())
}

fn load_template_from_disk() -> Option<ShakeTemplate> {
    let path = template_path().ok()?;
    if !path.exists() {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    if bytes.len() < 8 {
        return None;
    }
    let width = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let height = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let expected = 8 + (width as usize) * (height as usize) * 3;
    if bytes.len() != expected {
        return None;
    }
    Some(ShakeTemplate {
        width,
        height,
        rgb: bytes[8..].to_vec(),
    })
}

#[tauri::command]
fn load_regions() -> Result<RegionsConfig, String> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(RegionsConfig::default());
    }
    let bytes = fs::read(&path).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_regions(config: RegionsConfig) -> Result<(), String> {
    let path = config_path()?;
    let json = serde_json::to_vec_pretty(&config).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_screen_size() -> Result<(u32, u32), String> {
    let monitors = Monitor::all().map_err(|e| e.to_string())?;
    let monitor = monitors.first().ok_or("No monitor found")?;
    let w = monitor.width().map_err(|e| e.to_string())?;
    let h = monitor.height().map_err(|e| e.to_string())?;
    Ok((w, h))
}

/// On macOS Retina, xcap reports `monitor.width()` in LOGICAL pixels
/// (e.g. 1710) but the captured image is in PHYSICAL pixels (3420 wide).
/// Callers want to think in logical coordinates everywhere — same units as
/// the overlay, mouse cursor, click events, etc — so we expose the DPR
/// ratio and have callers multiply their logical coords by DPR when
/// indexing into the image buffer.
fn capture_with_dpr() -> Result<(xcap::image::RgbaImage, u32), String> {
    let monitors = Monitor::all().map_err(|e| e.to_string())?;
    let monitor = monitors.first().ok_or("No monitor found")?;
    let mw = monitor.width().map_err(|e| e.to_string())?;
    let image = monitor.capture_image().map_err(|e| e.to_string())?;
    let dpr = if mw == 0 { 1 } else { image.width() / mw };
    Ok((image, dpr.max(1)))
}

#[tauri::command]
fn mouse_move(x: i32, y: i32) -> Result<(), String> {
    // (x, y) are LOGICAL pixels — same units enigo and Core Graphics use,
    // so no conversion needed.
    #[cfg(target_os = "macos")]
    {
        cg_mouse::warp(x as f64, y as f64)?;
        return Ok(());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
        enigo
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[tauri::command]
fn click_at(x: i32, y: i32) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // Warp first so the on-screen cursor is in the right place, then
        // post mouse events at that exact position. We bypass enigo on
        // macOS because it tracks its own internal cursor position
        // (initialized to (0, 0) on every fresh `Enigo::new`) — sending
        // events through enigo would fire them at (0, 0), not at (x, y).
        cg_mouse::warp(x as f64, y as f64)?;
        // Brief pause so the windowserver has registered the warp before
        // we start posting button events.
        std::thread::sleep(std::time::Duration::from_millis(20));
        cg_mouse::left_button_down_at(x as f64, y as f64)?;
        // 80ms hold — short single-frame clicks are sometimes ignored by
        // game UI buttons that look for a sustained press.
        std::thread::sleep(std::time::Duration::from_millis(80));
        cg_mouse::left_button_up_at(x as f64, y as f64)?;
        return Ok(());
    }

    #[cfg(not(target_os = "macos"))]
    {
        let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
        enigo
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(|e| e.to_string())?;
        enigo
            .button(Button::Left, Direction::Press)
            .map_err(|e| e.to_string())?;
        std::thread::sleep(std::time::Duration::from_millis(40));
        enigo
            .button(Button::Left, Direction::Release)
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[tauri::command]
fn mouse_down() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        cg_mouse::left_button_down()?;
        return Ok(());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
        enigo
            .button(Button::Left, Direction::Press)
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// Start a background thread that taps Enter every `interval_ms` until
/// `stop_enter_spam` is called. Idempotent — calling while already running
/// just updates the interval.
#[tauri::command]
fn start_enter_spam(interval_ms: u64) -> Result<(), String> {
    ENTER_SPAM_INTERVAL_MS.store(interval_ms.max(10), Ordering::Relaxed);
    if ENTER_SPAM_RUNNING.swap(true, Ordering::SeqCst) {
        // Already running — caller has just updated the live interval.
        return Ok(());
    }
    thread::spawn(|| {
        while ENTER_SPAM_RUNNING.load(Ordering::SeqCst) {
            #[cfg(target_os = "macos")]
            {
                let _ = cg_mouse::press_return();
            }
            let ms = ENTER_SPAM_INTERVAL_MS.load(Ordering::Relaxed);
            thread::sleep(Duration::from_millis(ms));
        }
    });
    Ok(())
}

#[tauri::command]
fn stop_enter_spam() -> Result<(), String> {
    ENTER_SPAM_RUNNING.store(false, Ordering::SeqCst);
    Ok(())
}

/// Start a background thread that toggles the left mouse button every
/// `interval_ms`. Used for the "click" mode of fish-bar tracking — in
/// Roblox Fisch, alternating M1 down/up at high frequency holds the player
/// marker roughly stationary (between full-hold-rightward and
/// full-release-leftward). Cleans up by ensuring M1 is released on stop.
#[tauri::command]
fn start_m1_rapid_click(interval_ms: u64) -> Result<(), String> {
    M1_CLICK_INTERVAL_MS.store(interval_ms.max(10), Ordering::Relaxed);
    if M1_CLICK_RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    thread::spawn(|| {
        let mut held = false;
        while M1_CLICK_RUNNING.load(Ordering::SeqCst) {
            #[cfg(target_os = "macos")]
            {
                let _ = if held {
                    cg_mouse::left_button_up()
                } else {
                    cg_mouse::left_button_down()
                };
            }
            held = !held;
            let ms = M1_CLICK_INTERVAL_MS.load(Ordering::Relaxed);
            thread::sleep(Duration::from_millis(ms));
        }
        // Always end with M1 released so we leave the system in a clean state.
        #[cfg(target_os = "macos")]
        {
            let _ = cg_mouse::left_button_up();
        }
    });
    Ok(())
}

#[tauri::command]
fn stop_m1_rapid_click() -> Result<(), String> {
    M1_CLICK_RUNNING.store(false, Ordering::SeqCst);
    Ok(())
}

/// Start the PWM-style M1 control thread. JS sets `duty_pct` each tick
/// (0-100); the Rust thread presses M1 for `duty_pct/100 * cycle_ms` and
/// releases for the remainder, repeating every `cycle_ms`. Continuous
/// proportional input means continuous bar response — no mode-transition
/// overshoot.
#[tauri::command]
fn start_m1_pwm(duty_pct: u8, cycle_ms: u64) -> Result<(), String> {
    M1_PWM_DUTY.store(duty_pct.min(100) as u64, Ordering::Relaxed);
    M1_PWM_CYCLE_MS.store(cycle_ms.clamp(10, 500), Ordering::Relaxed);
    if M1_PWM_RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    thread::spawn(|| {
        while M1_PWM_RUNNING.load(Ordering::SeqCst) {
            let cycle = M1_PWM_CYCLE_MS.load(Ordering::Relaxed).max(10);
            let duty = M1_PWM_DUTY.load(Ordering::Relaxed).min(100);
            let on_ms = (cycle * duty) / 100;
            let off_ms = cycle.saturating_sub(on_ms);
            // Edge cases: 0% duty = always up (skip the down); 100% = always
            // down (skip the up). Avoids a needless toggle that would cost
            // a CGEvent post per cycle for no reason.
            if on_ms > 0 {
                #[cfg(target_os = "macos")]
                {
                    let _ = cg_mouse::left_button_down();
                }
                thread::sleep(Duration::from_millis(on_ms));
            }
            if off_ms > 0 {
                #[cfg(target_os = "macos")]
                {
                    let _ = cg_mouse::left_button_up();
                }
                thread::sleep(Duration::from_millis(off_ms));
            }
        }
        // Clean exit: always release M1.
        #[cfg(target_os = "macos")]
        {
            let _ = cg_mouse::left_button_up();
        }
    });
    Ok(())
}

#[tauri::command]
fn stop_m1_pwm() -> Result<(), String> {
    M1_PWM_RUNNING.store(false, Ordering::SeqCst);
    Ok(())
}

/// Update the PWM duty cycle without restarting the thread. Called every
/// detection tick from JS with the new controller-computed duty.
#[tauri::command]
fn set_m1_pwm_duty(duty_pct: u8) -> Result<(), String> {
    M1_PWM_DUTY.store(duty_pct.min(100) as u64, Ordering::Relaxed);
    Ok(())
}

/// Returns the absolute path to the per-run frames directory under the
/// user's Desktop (`~/Desktop/fisch-macro-debug/frames/run-<ts>`). JS
/// passes this to the tick_macro frame dumper as the dump_frame_path
/// parent. We don't mkdir here — the dumper does it lazily on first save,
/// so an empty run produces no folder.
#[tauri::command]
fn get_frames_run_dir(run_ts: u64) -> Result<String, String> {
    let mut dir = dirs::desktop_dir().ok_or("No desktop dir")?;
    dir.push("fisch-macro-debug");
    dir.push("frames");
    dir.push(format!("run-{}", run_ts));
    Ok(dir.to_string_lossy().to_string())
}

/// Open a fresh debug log file for the current session and start recording.
/// Returns the absolute path so JS can show it in the UI. Subsequent calls
/// while a log is already open close the previous file and start a new one.
#[tauri::command]
fn debug_log_start() -> Result<String, String> {
    let mut dir = dirs::desktop_dir().ok_or("No desktop dir")?;
    dir.push("fisch-macro-debug");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("macro-log-{}.txt", ts));
    let f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    *debug_log_lock().lock().unwrap() = Some(DebugLog {
        file: f,
        started_at: Instant::now(),
    });
    // Write a small header so the user (and future-me) knows what this is.
    debug_log_append(format!(
        "=== fisch-macro debug log {} ===",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    ))
    .ok();
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
fn debug_log_stop() -> Result<(), String> {
    *debug_log_lock().lock().unwrap() = None;
    Ok(())
}

#[tauri::command]
fn debug_log_append(line: String) -> Result<(), String> {
    let mut guard = debug_log_lock().lock().unwrap();
    if let Some(log) = guard.as_mut() {
        let elapsed = log.started_at.elapsed().as_secs_f64();
        writeln!(log.file, "[+{:7.3}s] {}", elapsed, line).map_err(|e| e.to_string())?;
        log.file.flush().ok();
    }
    Ok(())
}

#[tauri::command]
fn mouse_up() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        cg_mouse::left_button_up()?;
        return Ok(());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
        enigo
            .button(Button::Left, Direction::Release)
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// Captures the region and returns the absolute screen X of the brightest
/// column (sum of R+G+B per column). Used to locate the white player marker
/// inside the fish-bar minigame.
#[tauri::command]
fn find_player_x(x: i32, y: i32, width: u32, height: u32) -> Result<Option<i32>, String> {
    let (image, dpr) = capture_with_dpr()?;
    let img_w = (image.width() / dpr) as i32;
    let img_h = (image.height() / dpr) as i32;

    let x_start = x.max(0).min(img_w);
    let y_start = y.max(0).min(img_h);
    let x_end = (x + width as i32).max(0).min(img_w);
    let y_end = (y + height as i32).max(0).min(img_h);

    if x_end <= x_start || y_end <= y_start {
        return Ok(None);
    }

    let mut best_x = x_start;
    let mut best_score: u64 = 0;

    for px in x_start..x_end {
        let mut col_score: u64 = 0;
        for py in y_start..y_end {
            let p = image.get_pixel((px as u32) * dpr, (py as u32) * dpr);
            let r = p[0] as u64;
            let g = p[1] as u64;
            let b = p[2] as u64;
            let mn = r.min(g).min(b);
            let mx = r.max(g).max(b);
            let whiteness = if mx == 0 { 0 } else { (mn * 255) / mx };
            col_score += (r + g + b) * whiteness / 255;
        }
        if col_score > best_score {
            best_score = col_score;
            best_x = px;
        }
    }

    if best_score == 0 {
        Ok(None)
    } else {
        Ok(Some(best_x))
    }
}

#[derive(Serialize)]
struct ShakeResult {
    centroid: Option<(i32, i32)>,
    score: u64,         // template SAD at best position (lower = better)
    threshold: u64,     // max SAD that counts as a match
    has_template: bool, // false if no template captured yet
}

/// Sliding-window densest-cluster detector. Builds a dark-pixel mask for the
/// region, then slides a square window to find the area with the highest
/// concentration of dark pixels — this is the SHAKE button's tightly packed
/// circle. The centroid of dark pixels *within that window* gives the click
/// point.
///
/// Plain centroid-of-all-darks fails because background scenery contributes
/// thousands of scattered dark pixels that bias the mean toward a fixed
/// background point.
///
/// Window size scales with region dimensions: `min(w, h) / 3`, clamped to
/// [60, 400] physical pixels so it can fit a typical SHAKE button.
fn in_any_exclude(x: i32, y: i32, excludes: &[(i32, i32, i32, i32)]) -> bool {
    for (x1, y1, x2, y2) in excludes {
        if x >= *x1 && x < *x2 && y >= *y1 && y < *y2 {
            return true;
        }
    }
    false
}

#[allow(dead_code)] // legacy fallback; structural matcher above is now used instead
fn analyze_shake<F: Fn(u32, u32) -> [u8; 4]>(
    pixel_at: F,
    region_x: i32,
    region_y: i32,
    region_w: u32,
    region_h: u32,
    threshold: u8,
    min_window_pixels: u32,
    excludes: &[(i32, i32, i32, i32)], // list of (x1, y1, x2, y2) absolute
) -> (Option<(i32, i32)>, u32, u32) {
    if region_w == 0 || region_h == 0 {
        return (None, 0, 0);
    }
    let w = region_w as usize;
    let h = region_h as usize;

    // Dark mask + total count. Pixels inside any `excludes` rect are forced to 0.
    let mut mask = vec![0u32; w * h];
    let mut total_dark: u32 = 0;
    let t = (threshold as u32) * 3;
    for py in 0..region_h {
        for px in 0..region_w {
            let abs_x = region_x + px as i32;
            let abs_y = region_y + py as i32;
            if in_any_exclude(abs_x, abs_y, excludes) {
                continue;
            }
            let p = pixel_at(abs_x as u32, abs_y as u32);
            let bright = p[0] as u32 + p[1] as u32 + p[2] as u32;
            if bright < t {
                mask[py as usize * w + px as usize] = 1;
                total_dark += 1;
            }
        }
    }

    // Integral image of mask
    let iw = w + 1;
    let mut ii = vec![0u32; iw * (h + 1)];
    for py in 1..=h {
        for px in 1..=w {
            ii[py * iw + px] = mask[(py - 1) * w + (px - 1)]
                + ii[py * iw + (px - 1)]
                + ii[(py - 1) * iw + px]
                - ii[(py - 1) * iw + (px - 1)];
        }
    }

    let win = ((region_w.min(region_h)) / 3).clamp(60, 400) as usize;
    if win == 0 || win > w || win > h {
        return (None, total_dark, 0);
    }

    // Slide window
    let mut best_count: u32 = 0;
    let mut best_x: usize = 0;
    let mut best_y: usize = 0;
    let max_y = h - win;
    let max_x = w - win;
    for py in 0..=max_y {
        for px in 0..=max_x {
            // Window sum via integral image: A + D - B - C, but evaluated as
            // (A + D) - (B + C) so each partial step stays non-negative.
            let a = ii[(py + win) * iw + (px + win)];
            let b = ii[py * iw + (px + win)];
            let c = ii[(py + win) * iw + px];
            let d = ii[py * iw + px];
            let count = (a + d) - (b + c);
            if count > best_count {
                best_count = count;
                best_x = px;
                best_y = py;
            }
        }
    }

    let _ = min_window_pixels; // see analyze_shake_diff: caller gates on best_count
    if best_count == 0 {
        return (None, total_dark, best_count);
    }
    let mut sx: u64 = 0;
    let mut sy: u64 = 0;
    let mut cnt: u64 = 0;
    for py in best_y..(best_y + win) {
        for px in best_x..(best_x + win) {
            if mask[py * w + px] == 1 {
                sx += region_x as u64 + px as u64;
                sy += region_y as u64 + py as u64;
                cnt += 1;
            }
        }
    }
    if cnt == 0 {
        return (None, total_dark, best_count);
    }
    let cx = (sx / cnt) as i32;
    let cy = (sy / cnt) as i32;
    (Some((cx, cy)), total_dark, best_count)
}

/// Capture the pixels in the given rect as the SHAKE button template. The
/// rect should be tightly cropped around the SHAKE button while it's
/// visible in-game.
#[tauri::command]
fn capture_shake_template(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<u32, String> {
    if width == 0 || height == 0 {
        return Err("Empty template region".to_string());
    }

    // Use the same CG partial-capture path that tick_macro uses, so the
    // captured pixels exactly match what detection scans (no risk of xcap
    // and CG returning slightly different coordinate interpretations).
    #[cfg(target_os = "macos")]
    {
        let cap = cg_capture::capture_logical_rect(x, y, width, height)?;
        let dpr = (cap.width / width).max(1);
        let mut rgb = Vec::with_capacity((width as usize) * (height as usize) * 3);
        for py in 0..height {
            for px in 0..width {
                let pixel = cap.pixel_at(px * dpr, py * dpr);
                rgb.push(pixel[0]);
                rgb.push(pixel[1]);
                rgb.push(pixel[2]);
            }
        }
        let template = ShakeTemplate {
            width,
            height,
            rgb,
        };
        let _ = save_template_to_disk(&template);
        *template_lock().lock().unwrap() = Some(template);
        return Ok(width * height);
    }

    #[cfg(not(target_os = "macos"))]
    {
        let (image, dpr) = capture_with_dpr()?;
        let img_w = (image.width() / dpr) as i32;
        let img_h = (image.height() / dpr) as i32;
        let xs = x.max(0).min(img_w);
        let ys = y.max(0).min(img_h);
        let xe = (x + width as i32).max(0).min(img_w);
        let ye = (y + height as i32).max(0).min(img_h);
        let rw = (xe - xs).max(0) as u32;
        let rh = (ye - ys).max(0) as u32;
        let mut rgb = Vec::with_capacity((rw as usize) * (rh as usize) * 3);
        for py in ys..ye {
            for px in xs..xe {
                let p = image.get_pixel((px as u32) * dpr, (py as u32) * dpr);
                rgb.push(p[0]);
                rgb.push(p[1]);
                rgb.push(p[2]);
            }
        }
        let template = ShakeTemplate {
            width: rw,
            height: rh,
            rgb,
        };
        let _ = save_template_to_disk(&template);
        *template_lock().lock().unwrap() = Some(template);
        Ok(rw * rh)
    }
}

#[tauri::command]
fn clear_shake_template() -> Result<(), String> {
    *template_lock().lock().unwrap() = None;
    if let Ok(path) = template_path() {
        let _ = fs::remove_file(&path);
    }
    Ok(())
}

#[tauri::command]
fn has_shake_template() -> bool {
    template_lock().lock().unwrap().is_some()
}

/// Debug: capture the full screen via CG (the same method tick_macro uses)
/// and save to disk so we can see what cg_capture is actually returning vs
/// what the user sees on screen. Helps diagnose coord-system mismatches.
#[tauri::command]
fn debug_save_cg_full() -> Result<String, String> {
    use image::{ImageBuffer, RgbImage};

    let monitors = Monitor::all().map_err(|e| e.to_string())?;
    let monitor = monitors.first().ok_or("No monitor found")?;
    let mw = monitor.width().map_err(|e| e.to_string())?;
    let mh = monitor.height().map_err(|e| e.to_string())?;

    #[cfg(target_os = "macos")]
    {
        let cg = cg_capture::capture_logical_rect(0, 0, mw, mh)?;
        let mut rgb = Vec::with_capacity((cg.width as usize) * (cg.height as usize) * 3);
        for py in 0..cg.height {
            for px in 0..cg.width {
                let p = cg.pixel_at(px, py);
                rgb.push(p[0]);
                rgb.push(p[1]);
                rgb.push(p[2]);
            }
        }
        let img: RgbImage = ImageBuffer::from_raw(cg.width, cg.height, rgb)
            .ok_or("image build failed")?;

        let mut path = dirs::desktop_dir().ok_or("No desktop dir")?;
        path.push("fisch-macro-debug");
        fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        path.push("debug_cg_full.png");
        img.save(&path).map_err(|e| e.to_string())?;
        return Ok(format!(
            "saved {}×{} (logical {}×{}) to {}",
            cg.width,
            cg.height,
            mw,
            mh,
            path.to_string_lossy()
        ));
    }

    #[cfg(not(target_os = "macos"))]
    Err("debug_save_cg_full only on macOS".to_string())
}

/// Debug: capture the full screen and draw a red rect at (x, y, w, h)
/// LOGICAL pixels — the EXACT rect the macro thinks it's capturing for the
/// template. Saves under `template_marker_{suffix}.png` so we can compare
/// against the on-screen position the user drew.
#[tauri::command]
fn debug_save_full_with_marker(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    suffix: String,
) -> Result<String, String> {
    use image::{ImageBuffer, RgbImage};

    let monitors = Monitor::all().map_err(|e| e.to_string())?;
    let monitor = monitors.first().ok_or("No monitor found")?;
    let mw = monitor.width().map_err(|e| e.to_string())?;
    let mh = monitor.height().map_err(|e| e.to_string())?;

    #[cfg(target_os = "macos")]
    {
        let cap = cg_capture::capture_logical_rect(0, 0, mw, mh)?;
        let dpr = if mw == 0 { 1 } else { (cap.width / mw).max(1) };

        let mut rgb = Vec::with_capacity((cap.width as usize) * (cap.height as usize) * 3);
        for py in 0..cap.height {
            for px in 0..cap.width {
                let p = cap.pixel_at(px, py);
                rgb.push(p[0]);
                rgb.push(p[1]);
                rgb.push(p[2]);
            }
        }

        // Draw a red rect at the logical (x, y, w, h) → physical pixels.
        let px_x = x.max(0) as u32 * dpr;
        let px_y = y.max(0) as u32 * dpr;
        let px_w = width * dpr;
        let px_h = height * dpr;
        let thickness = 4u32;
        let stride = cap.width;

        let mut paint = |xx: u32, yy: u32| {
            if xx >= cap.width || yy >= cap.height {
                return;
            }
            let idx = ((yy * stride + xx) * 3) as usize;
            rgb[idx] = 255;
            rgb[idx + 1] = 0;
            rgb[idx + 2] = 0;
        };

        for t in 0..thickness {
            for xi in 0..px_w {
                paint(px_x + xi, px_y + t);
                if px_h > t {
                    paint(px_x + xi, px_y + px_h - 1 - t);
                }
            }
            for yi in 0..px_h {
                paint(px_x + t, px_y + yi);
                if px_w > t {
                    paint(px_x + px_w - 1 - t, px_y + yi);
                }
            }
        }

        let img: RgbImage = ImageBuffer::from_raw(cap.width, cap.height, rgb)
            .ok_or("image build failed")?;

        let mut path = dirs::desktop_dir().ok_or("No desktop dir")?;
        path.push("fisch-macro-debug");
        fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        let safe_suffix: String = suffix
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        path.push(format!("template_marker_{}.png", safe_suffix));
        img.save(&path).map_err(|e| e.to_string())?;
        return Ok(path.to_string_lossy().to_string());
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (x, y, width, height, suffix, mw, mh);
        Err("debug_save_full_with_marker only on macOS".to_string())
    }
}

/// Dump the stored template to ~/Desktop/fisch-macro-debug/shake_template.png
/// so the user can verify what was actually captured.
#[tauri::command]
fn save_shake_template_image() -> Result<String, String> {
    use image::{ImageBuffer, RgbImage};
    let guard = template_lock().lock().unwrap();
    let template = guard.as_ref().ok_or("No template captured")?;

    let mut dir = dirs::desktop_dir().ok_or("No desktop dir")?;
    dir.push("fisch-macro-debug");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("shake_template.png");

    let img: RgbImage =
        ImageBuffer::from_raw(template.width, template.height, template.rgb.clone())
            .ok_or("template image build failed")?;
    img.save(&path).map_err(|e| e.to_string())?;

    Ok(path.to_string_lossy().to_string())
}

/// Slide the template across the search region using NORMALIZED CROSS
/// CORRELATION (NCC) on per-cell average luminance.
///
/// Both the template and each candidate window are reduced to a 14×14 grid
/// of cell-averaged luminance values (each cell is the mean luminance of an
/// 8×8 pixel block). NCC then measures linear correlation between the two
/// vectors — invariant to absolute brightness shifts and overall gain
/// changes between when the template was captured and when matching runs.
/// That makes it robust to lighting differences (day/night, cliff/sky/water
/// background bleeding through a transparent button) that broke earlier
/// fixed-threshold approaches.
///
/// The 8×8 cell averaging gives sub-cell shift tolerance similar to a
/// max-pool — minor pixel offsets between capture and runtime get smoothed
/// out instead of dropping cells from a binary mask.
///
/// Score: `score = (1 - ncc) * 1000`. Lower = better.
/// Match if `score <= max_avg_diff * 10`. Real SHAKE matches typically
/// score 50–200 (NCC ≥ 0.8); unrelated scene textures score 500–800.
fn analyze_shake_template<F: Fn(u32, u32) -> [u8; 4]>(
    pixel_at: F,
    region_x: i32,
    region_y: i32,
    region_w: u32,
    region_h: u32,
    template: &ShakeTemplate,
    max_avg_diff: u32,
    excludes: &[(i32, i32, i32, i32)],
) -> (Option<(i32, i32)>, u64, u64) {
    let tw = template.width;
    let th = template.height;
    if tw < 8 || th < 8 || tw > region_w || th > region_h {
        return (None, 2000, 0);
    }

    let s: u32 = 8;
    let stw = tw / s;
    let sth = th / s;
    if stw == 0 || sth == 0 {
        return (None, 2000, 0);
    }
    let n = (stw as usize) * (sth as usize);

    // ---- Build template per-cell average luminance ----
    let mut t_avg = vec![0f64; n];
    let cell_area_f = (s * s) as f64;
    for ty in 0..sth {
        for tx in 0..stw {
            let mut sum = 0f64;
            for cy in 0..s {
                for cx in 0..s {
                    let py = ty * s + cy;
                    let px = tx * s + cx;
                    let idx = ((py * tw + px) * 3) as usize;
                    let lum = 0.299 * template.rgb[idx] as f64
                        + 0.587 * template.rgb[idx + 1] as f64
                        + 0.114 * template.rgb[idx + 2] as f64;
                    sum += lum;
                }
            }
            t_avg[(ty * stw + tx) as usize] = sum / cell_area_f;
        }
    }

    // Center & normalize template so we can compute NCC numerator/denominator
    // in one pass per candidate window.
    let t_mean: f64 = t_avg.iter().sum::<f64>() / (n as f64);
    let t_centered: Vec<f64> = t_avg.iter().map(|&v| v - t_mean).collect();
    let t_norm: f64 = t_centered.iter().map(|&v| v * v).sum::<f64>().sqrt();
    if t_norm < 1.0 {
        // Template has no luminance variation (uniform) — cannot produce
        // a meaningful correlation.
        return (None, 2000, (max_avg_diff as u64) * 10);
    }

    // Bright-cell bounding-box for the template, used as a SHAPE sanity
    // check below. Without this, NCC alone matches anything with a similar
    // luminance pattern — including the player nametag, level text, and
    // other bright-on-dark UI strips that aren't actually circular buttons.
    // We threshold conservatively (180) so this only flags clearly bright
    // cells (button outline + text), not mid-luminance background.
    let bright_cell_threshold: f64 = 180.0;
    let mut t_min_x: u32 = stw;
    let mut t_max_x: u32 = 0;
    let mut t_min_y: u32 = sth;
    let mut t_max_y: u32 = 0;
    let mut t_bright_count: u32 = 0;
    for ty in 0..sth {
        for tx in 0..stw {
            if t_avg[(ty * stw + tx) as usize] > bright_cell_threshold {
                if tx < t_min_x { t_min_x = tx; }
                if tx > t_max_x { t_max_x = tx; }
                if ty < t_min_y { t_min_y = ty; }
                if ty > t_max_y { t_max_y = ty; }
                t_bright_count += 1;
            }
        }
    }
    let (t_aspect, t_has_shape) = if t_bright_count >= 4 {
        let w = (t_max_x as i32 - t_min_x as i32 + 1).max(1) as f64;
        let h = (t_max_y as i32 - t_min_y as i32 + 1).max(1) as f64;
        (w / h, true)
    } else {
        (1.0, false)
    };

    // Vertical-symmetry fingerprint of the template's bright cells. The SHAKE
    // button is roughly top-bottom symmetric (the outline ring's upper arc has
    // about the same number of bright cells as the lower arc). The player
    // character is the opposite — very top-heavy (bright pumpkin head, dark
    // legs). Recording what fraction of the template's bright cells live in
    // the *upper* half lets us reject candidates whose vertical distribution
    // is way off, even when their bounding-box aspect happens to be square.
    let t_upper_frac = if t_has_shape {
        let t_mid_y = (t_min_y + t_max_y) / 2;
        let mut upper = 0u32;
        let mut total = 0u32;
        for ty in t_min_y..=t_max_y {
            for tx in t_min_x..=t_max_x {
                if t_avg[(ty * stw + tx) as usize] > bright_cell_threshold {
                    total += 1;
                    if ty < t_mid_y {
                        upper += 1;
                    }
                }
            }
        }
        if total > 0 {
            upper as f64 / total as f64
        } else {
            0.5
        }
    } else {
        0.5
    };

    // ---- Build search-region per-pixel luminance + integral image, so
    // we can query the average luminance of any 8×8 cell in O(1). ----
    let s_total = (region_w * region_h) as usize;
    let mut s_lum = vec![0f64; s_total];
    for ry in 0..region_h {
        for rx in 0..region_w {
            let p = pixel_at(
                (region_x + rx as i32) as u32,
                (region_y + ry as i32) as u32,
            );
            s_lum[(ry * region_w + rx) as usize] =
                0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
        }
    }
    let iw = (region_w + 1) as usize;
    let mut ii = vec![0f64; iw * (region_h as usize + 1)];
    for ry in 1..=region_h as usize {
        for rx in 1..=region_w as usize {
            let val = s_lum[(ry - 1) * region_w as usize + (rx - 1)];
            ii[ry * iw + rx] = val
                + ii[ry * iw + (rx - 1)]
                + ii[(ry - 1) * iw + rx]
                - ii[(ry - 1) * iw + (rx - 1)];
        }
    }
    let cell_avg = |rx: u32, ry: u32| -> f64 {
        let ax = rx as usize;
        let ay = ry as usize;
        let bx = (rx + s) as usize;
        let by = (ry + s) as usize;
        (ii[by * iw + bx] + ii[ay * iw + ax]
            - ii[by * iw + ax]
            - ii[ay * iw + bx])
            / cell_area_f
    };

    let max_x = region_w - tw;
    let max_y = region_h - th;
    let threshold_score = (max_avg_diff as u64) * 10;

    let mut best_ncc: f64 = -1.0;
    let mut best_x = 0u32;
    let mut best_y = 0u32;

    let mut p_avg = vec![0f64; n];

    let mut sy = 0u32;
    while sy <= max_y {
        let mut sx = 0u32;
        while sx <= max_x {
            let t_x1 = region_x + sx as i32;
            let t_y1 = region_y + sy as i32;
            let t_x2 = t_x1 + tw as i32;
            let t_y2 = t_y1 + th as i32;
            let mut excluded = false;
            for &(x1, y1, x2, y2) in excludes {
                if t_x1 < x2 && t_x2 > x1 && t_y1 < y2 && t_y2 > y1 {
                    excluded = true;
                    break;
                }
            }
            if excluded {
                sx += s;
                continue;
            }

            let mut p_sum = 0f64;
            let mut p_min_x: u32 = stw;
            let mut p_max_x: u32 = 0;
            let mut p_min_y: u32 = sth;
            let mut p_max_y: u32 = 0;
            let mut p_bright_count: u32 = 0;
            for ty in 0..sth {
                for tx in 0..stw {
                    let cx = sx + tx * s;
                    let cy = sy + ty * s;
                    let avg = cell_avg(cx, cy);
                    p_avg[(ty * stw + tx) as usize] = avg;
                    p_sum += avg;
                    if avg > bright_cell_threshold {
                        if tx < p_min_x { p_min_x = tx; }
                        if tx > p_max_x { p_max_x = tx; }
                        if ty < p_min_y { p_min_y = ty; }
                        if ty > p_max_y { p_max_y = ty; }
                        p_bright_count += 1;
                    }
                }
            }

            // SHAPE sanity checks: reject candidates whose bright cells form
            // a region that's structurally different from the template's.
            //   1. ASPECT — kills wide horizontal strips (notification banners,
            //      nametag + level lines).
            //   2. VERTICAL SYMMETRY — kills top-heavy or bottom-heavy patterns
            //      (the player character: bright head, dark legs). The SHAKE
            //      button's outline is symmetric top↔bottom so its upper-half
            //      bright fraction is ~0.5; the player's is ~0.7+.
            if t_has_shape && p_bright_count >= 4 {
                let p_w = (p_max_x as i32 - p_min_x as i32 + 1).max(1) as f64;
                let p_h = (p_max_y as i32 - p_min_y as i32 + 1).max(1) as f64;
                let p_aspect = p_w / p_h;
                let aspect_diff = if p_aspect > t_aspect {
                    p_aspect / t_aspect
                } else {
                    t_aspect / p_aspect
                };
                if aspect_diff > 2.0 {
                    sx += s;
                    continue;
                }

                // Compute upper-half fraction of patch's bright cells.
                let p_mid_y = (p_min_y + p_max_y) / 2;
                let mut p_upper = 0u32;
                let mut p_total = 0u32;
                for ty in p_min_y..=p_max_y {
                    for tx in p_min_x..=p_max_x {
                        if p_avg[(ty * stw + tx) as usize] > bright_cell_threshold {
                            p_total += 1;
                            if ty < p_mid_y {
                                p_upper += 1;
                            }
                        }
                    }
                }
                let p_upper_frac = if p_total > 0 {
                    p_upper as f64 / p_total as f64
                } else {
                    0.5
                };
                if (p_upper_frac - t_upper_frac).abs() > 0.25 {
                    sx += s;
                    continue;
                }
            }

            let p_mean = p_sum / (n as f64);

            let mut dot = 0f64;
            let mut p_norm_sq = 0f64;
            for k in 0..n {
                let di = p_avg[k] - p_mean;
                dot += di * t_centered[k];
                p_norm_sq += di * di;
            }
            let p_norm = p_norm_sq.sqrt();
            let ncc = if p_norm < 1.0 {
                0.0
            } else {
                dot / (t_norm * p_norm)
            };

            if ncc > best_ncc {
                best_ncc = ncc;
                best_x = sx;
                best_y = sy;
            }
            sx += s;
        }
        sy += s;
    }

    let best_score = if best_ncc < -1.0 {
        2000
    } else {
        ((1.0 - best_ncc) * 1000.0).round().clamp(0.0, 2000.0) as u64
    };

    let centroid = if best_score <= threshold_score {
        let cx = region_x + best_x as i32 + (tw / 2) as i32;
        let cy = region_y + best_y as i32 + (th / 2) as i32;
        Some((cx, cy))
    } else {
        None
    };

    (centroid, best_score, threshold_score)
}

/// Saves debug snapshots of the shake region as PNGs to the user's Desktop:
///   - shake_current.png   — what the macro sees right now
///   - shake_baseline.png  — what was captured as baseline (if any)
///   - shake_diff.png      — visualization where changed pixels are red,
///                           non-changed pixels are dimmed original
/// Returns the directory path where files were written.
#[tauri::command]
fn save_shake_snapshot(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    diff_threshold: u32,
    excludes: Option<Vec<Region>>,
) -> Result<String, String> {
    use image::{ImageBuffer, RgbImage};

    let mut dir = dirs::desktop_dir().ok_or("No desktop dir")?;
    dir.push("fisch-macro-debug");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let (captured, dpr) = capture_with_dpr()?;
    let img_w = (captured.width() / dpr) as i32;
    let img_h = (captured.height() / dpr) as i32;
    let xs = x.max(0).min(img_w);
    let ys = y.max(0).min(img_h);
    let xe = (x + width as i32).max(0).min(img_w);
    let ye = (y + height as i32).max(0).min(img_h);
    let rw = (xe - xs) as u32;
    let rh = (ye - ys) as u32;
    if rw == 0 || rh == 0 {
        return Err("Empty region".to_string());
    }

    // Build current-frame RGB buffer
    let mut current = Vec::with_capacity((rw * rh * 3) as usize);
    for py in ys..ye {
        for px in xs..xe {
            let p = captured.get_pixel((px as u32) * dpr, (py as u32) * dpr);
            current.push(p[0]);
            current.push(p[1]);
            current.push(p[2]);
        }
    }

    let cur_img: RgbImage =
        ImageBuffer::from_raw(rw, rh, current.clone()).ok_or("current image build failed")?;
    cur_img
        .save(dir.join("shake_current.png"))
        .map_err(|e| format!("save current: {}", e))?;

    if let Some(baseline) = baseline_lock().lock().unwrap().as_ref() {
        if baseline.width == rw && baseline.height == rh {
            let base_img: RgbImage = ImageBuffer::from_raw(rw, rh, baseline.rgb.clone())
                .ok_or("baseline image build failed")?;
            base_img
                .save(dir.join("shake_baseline.png"))
                .map_err(|e| format!("save baseline: {}", e))?;

            // Build the same combined exclude list the detector uses, so the
            // diff visualization matches what the algorithm actually sees.
            let mut all_excludes: Vec<(i32, i32, i32, i32)> = excludes
                .unwrap_or_default()
                .into_iter()
                .map(|r| (r.x, r.y, r.x + r.width as i32, r.y + r.height as i32))
                .collect();
            for &rect in &baseline.excludes_at_capture {
                all_excludes.push(rect);
            }

            // Diff visualization. Excluded pixels render as solid grey so they
            // are visibly tagged "ignored" rather than confused with red diff.
            let mut diff = Vec::with_capacity((rw * rh * 3) as usize);
            for py in 0..rh {
                for px in 0..rw {
                    let abs_x = xs + px as i32;
                    let abs_y = ys + py as i32;
                    let i = (py * rw + px) as usize;
                    if in_any_exclude(abs_x, abs_y, &all_excludes) {
                        diff.push(80);
                        diff.push(80);
                        diff.push(80);
                        continue;
                    }
                    let cr = current[i * 3] as i32;
                    let cg = current[i * 3 + 1] as i32;
                    let cb = current[i * 3 + 2] as i32;
                    let br = baseline.rgb[i * 3] as i32;
                    let bg = baseline.rgb[i * 3 + 1] as i32;
                    let bb = baseline.rgb[i * 3 + 2] as i32;
                    let d = (cr - br).unsigned_abs()
                        + (cg - bg).unsigned_abs()
                        + (cb - bb).unsigned_abs();
                    if d >= diff_threshold {
                        diff.push(255);
                        diff.push(0);
                        diff.push(0);
                    } else {
                        diff.push((cr / 3) as u8);
                        diff.push((cg / 3) as u8);
                        diff.push((cb / 3) as u8);
                    }
                }
            }
            let diff_img: RgbImage =
                ImageBuffer::from_raw(rw, rh, diff).ok_or("diff image build failed")?;
            diff_img
                .save(dir.join("shake_diff.png"))
                .map_err(|e| format!("save diff: {}", e))?;
        }
    }

    Ok(dir.to_string_lossy().to_string())
}

// Color calibration types. Returned to JS so the Fish panel can auto-fill
// hex inputs and also show every dominant color in the captured region for
// the user to manually pick from when classification is ambiguous.
#[derive(Serialize, Clone)]
struct DominantColor {
    hex: String,
    pixel_count: u32,
    classification: &'static str,
}

#[derive(Serialize, Default)]
struct CalibrationResult {
    dominants: Vec<DominantColor>,
    suggested_left_bar: Option<String>,
    suggested_right_bar: Option<String>,
    suggested_target_line: Option<String>,
    suggested_arrow: Option<String>,
    suggested_fish: Option<String>,
    has_saturated_blue: bool,
    region_pixels: u32,
    region_width: u32,
    region_height: u32,
    // Suggested detection thresholds based on observed pixel density.
    // The bar's vertical extent in the region determines the realistic
    // ceiling for "white % per column" — if bar is 25 px tall in a 70 px
    // region, no column can ever pass 80%, so we cap suggestion at
    // ceiling * 0.85 with margin.
    suggested_white_pct: Option<u8>,
    suggested_min_line_density: Option<u8>,
    // Tolerance suggestions. Auto-cal previously hardcoded tol=3 which
    // creates a "dead zone" between left/right hexes when they're more
    // than 6 units apart per channel — bar pixels in the gap match
    // neither. Suggested tol bridges the gap with a 2-unit margin.
    suggested_bar_tol: Option<u8>,
    suggested_target_tol: Option<u8>,
    // Diagnostics so the UI can explain the suggestion.
    bar_max_col_count: u32,
    target_max_col_count: u32,
    // Best blue found by full-pixel-buffer scan, even if not in top-30.
    deep_blue_hex: Option<String>,
    deep_blue_count: u32,
}

/// Analyze the fish_bar region and return dominant colors + best-guess
/// classification for each role (left/right bar, target line, arrow,
/// fish-indicator). Frontend uses this to auto-fill the Color Options
/// hex fields without the user having to inspect a PNG manually.
///
/// Algorithm:
///   1. Capture region (no-op for empty regions).
///   2. Quantize each pixel to a 16-step bucket per channel (4096 buckets).
///   3. Build a histogram, sort buckets by pixel count.
///   4. Classify the top 30 dominants against expected color signatures:
///      - "bar"   — near-white (mn ≥ 220, low saturation)
///      - "dark_blue" — dark navy/blue (mn ≤ 80, blue dominant, saturated)
///      - "arrow" — mid-grey (mn 40-160, low saturation)
///      - "fish"  — saturated blue (more saturated than dark_blue, mid-bright)
///      - "track" — very dark (mn < 40)
///      - "other" — none of the above
///   5. Suggest the highest-count exemplar of each role.
#[tauri::command]
fn calibrate_fish_colors(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<CalibrationResult, String> {
    use std::collections::HashMap;

    // Pull pixels into a single Vec<(u8,u8,u8)> for the analysis loop;
    // platform code paths differ but feed the same buffer.
    let mut rgb: Vec<(u8, u8, u8)> = Vec::new();

    #[cfg(target_os = "macos")]
    {
        let cap = cg_capture::capture_logical_rect(x, y, width, height)?;
        let req_w = width.max(1);
        let dpr = (cap.width / req_w).max(1);
        rgb.reserve((width * height) as usize);
        for py in 0..height {
            for px in 0..width {
                let p = cap.pixel_at(px * dpr, py * dpr);
                rgb.push((p[0], p[1], p[2]));
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let (captured, dpr) = capture_with_dpr()?;
        let img_w = (captured.width() / dpr) as i32;
        let img_h = (captured.height() / dpr) as i32;
        let xs = x.max(0).min(img_w);
        let ys = y.max(0).min(img_h);
        let xe = (x + width as i32).max(0).min(img_w);
        let ye = (y + height as i32).max(0).min(img_h);
        for py in ys..ye {
            for px in xs..xe {
                let p = captured.get_pixel((px as u32) * dpr, (py as u32) * dpr);
                rgb.push((p[0], p[1], p[2]));
            }
        }
    }

    if rgb.is_empty() {
        return Err("Empty region".to_string());
    }

    // Quantize to 16-step buckets — drops noise from anti-aliasing/JPEG-ish
    // game compression while keeping enough resolution to distinguish
    // navy from cyan, etc.
    const BUCKET: u8 = 16;
    let mut hist: HashMap<u32, u32> = HashMap::with_capacity(2048);
    for (r, g, b) in &rgb {
        let qr = (r / BUCKET) * BUCKET;
        let qg = (g / BUCKET) * BUCKET;
        let qb = (b / BUCKET) * BUCKET;
        let key = ((qr as u32) << 16) | ((qg as u32) << 8) | (qb as u32);
        *hist.entry(key).or_insert(0) += 1;
    }

    let mut sorted: Vec<(u32, u32)> = hist.into_iter().collect();
    sorted.sort_by_key(|(_, count)| std::cmp::Reverse(*count));

    let classify = |r: u8, g: u8, b: u8| -> &'static str {
        let mn = r.min(g).min(b) as i32;
        let mx = r.max(g).max(b) as i32;
        let spread = mx - mn;
        let bri = (r as i32 + g as i32 + b as i32) / 3;
        if mn >= 220 && spread <= 25 {
            "bar"
        } else if mn < 40 && spread <= 30 {
            "track"
        } else if (b as i32) > (r as i32) + 15
            && (b as i32) > (g as i32) + 5
            && bri < 110
            && spread >= 12
        {
            "dark_blue"
        } else if (b as i32) > (r as i32) + 25
            && (b as i32) > (g as i32) + 10
            && bri >= 80
            && bri <= 200
        {
            "fish"
        } else if mn >= 50 && mn <= 160 && spread <= 35 {
            "arrow"
        } else {
            "other"
        }
    };

    let dominants: Vec<DominantColor> = sorted
        .iter()
        .take(30)
        .map(|(key, count)| {
            let r = ((key >> 16) & 0xff) as u8;
            let g = ((key >> 8) & 0xff) as u8;
            let b = (key & 0xff) as u8;
            DominantColor {
                hex: format!("{:02x}{:02x}{:02x}", r, g, b),
                pixel_count: *count,
                classification: classify(r, g, b),
            }
        })
        .collect();

    // Resolve all picks up front (immutable borrows of `dominants`),
    // then we can move the vec into the result struct cleanly.
    let pick_first = |class: &str| -> Option<String> {
        dominants
            .iter()
            .find(|d| d.classification == class)
            .map(|d| d.hex.clone())
    };

    // For left/right, find the two brightest "bar" entries; lower-bri = left,
    // higher-bri = right. If there's only one (some rod variants render the
    // bar as a uniform color, not a gradient), synthesize a slightly-darker
    // companion so the two-color match still gives the detector slack
    // instead of collapsing into a single hex with double weight.
    let mut bar_entries: Vec<&DominantColor> = dominants
        .iter()
        .filter(|d| d.classification == "bar")
        .collect();
    bar_entries.sort_by_key(|d| {
        let r = u8::from_str_radix(&d.hex[0..2], 16).unwrap_or(0);
        let g = u8::from_str_radix(&d.hex[2..4], 16).unwrap_or(0);
        let b = u8::from_str_radix(&d.hex[4..6], 16).unwrap_or(0);
        r as u32 + g as u32 + b as u32
    });
    let left_bar: Option<String>;
    let right_bar: Option<String>;
    match bar_entries.len() {
        0 => {
            left_bar = None;
            right_bar = None;
        }
        1 => {
            // Only one bar shade — bar is rendered uniformly. Synthesize
            // a 16-step-darker companion as the "left" so the matcher still
            // has range; the user's actual bar pixels will all match the
            // single hex through right_tol either way.
            let h = &bar_entries[0].hex;
            let r = u8::from_str_radix(&h[0..2], 16).unwrap_or(0);
            let g = u8::from_str_radix(&h[2..4], 16).unwrap_or(0);
            let b = u8::from_str_radix(&h[4..6], 16).unwrap_or(0);
            let dim_r = r.saturating_sub(16);
            let dim_g = g.saturating_sub(16);
            let dim_b = b.saturating_sub(16);
            left_bar = Some(format!("{:02x}{:02x}{:02x}", dim_r, dim_g, dim_b));
            right_bar = Some(h.clone());
        }
        _ => {
            left_bar = Some(bar_entries.first().unwrap().hex.clone());
            right_bar = Some(bar_entries.last().unwrap().hex.clone());
        }
    };

    let target_line_dom = pick_first("dark_blue");
    let arrow = pick_first("arrow");
    let fish_dom = pick_first("fish");
    let has_saturated_blue =
        dominants.iter().any(|d| d.classification == "fish");
    drop(bar_entries);

    // Vertical-line scan: this is what makes auto-cal reliable on noisy
    // regions. The fish indicator is a thin vertical line — concentrated
    // in 1–3 columns, ~20–40 px tall. Background noise (avatar skin,
    // scenery) is spread broadly across many columns. So we score each
    // quantized color by:
    //   max_col_count = pixels of this color in its tallest column
    //   concentration = max_col_count / total_count
    // A real vertical line has high max_col_count *and* high concentration
    // (>0.4 typically). Random background has low max_col_count even if
    // total is high. We require the candidate to be:
    //   - Not "bar" white (mn < 200)
    //   - Not "track" dark (mn >= 30)
    //   - Reasonably saturated (not pure grey)
    //   - Located in the central 80% of the region (excludes edges where
    //     the player avatar / scenery typically lives)
    //
    // Fixes the bug from log 1777250611 where target=606070 was
    // matching avatar pixels at x=1180-1198, not the fish line.
    let cols_u = width as usize;
    let edge_skip = (cols_u as f64 * 0.10) as usize;
    let center_lo = edge_skip;
    let center_hi = cols_u.saturating_sub(edge_skip);

    let mut per_col_hist: std::collections::HashMap<u32, Vec<u32>> =
        std::collections::HashMap::new();
    for (i, (r, g, b)) in rgb.iter().enumerate() {
        let col = i % cols_u;
        if col < center_lo || col >= center_hi {
            continue;
        }
        let qr = (r / BUCKET) * BUCKET;
        let qg = (g / BUCKET) * BUCKET;
        let qb = (b / BUCKET) * BUCKET;
        let key = ((qr as u32) << 16) | ((qg as u32) << 8) | (qb as u32);
        let counts = per_col_hist
            .entry(key)
            .or_insert_with(|| vec![0u32; cols_u]);
        counts[col] += 1;
    }

    // Pick the best vertical-line candidate. Score = max_col_count when
    // the candidate passes the saturation + brightness gate. Prefer
    // colors that look like blue lines (b > r + 10).
    let mut best_line: Option<(u32, u32, [u8; 3])> = None; // (max_col, score, rgb)
    for (key, counts) in &per_col_hist {
        let r = ((key >> 16) & 0xff) as i32;
        let g = ((key >> 8) & 0xff) as i32;
        let b = (key & 0xff) as i32;
        let mn = r.min(g).min(b);
        let mx = r.max(g).max(b);
        let spread = mx - mn;
        // Skip whites, near-blacks, and totally-grey background.
        if mn >= 200 {
            continue;
        }
        if mn < 30 {
            continue;
        }
        if spread < 10 {
            continue;
        }
        let max_col_count = counts.iter().copied().max().unwrap_or(0);
        if max_col_count < 12 {
            continue;
        }
        let total: u32 = counts.iter().sum();
        let concentration = max_col_count as f64 / total.max(1) as f64;
        if concentration < 0.30 {
            continue;
        }
        let blue_bias = if b > r + 10 && b > g + 5 { 1.5 } else { 1.0 };
        let score = (max_col_count as f64 * blue_bias) as u32;
        if best_line.map_or(true, |(_, s, _)| score > s) {
            best_line = Some((max_col_count, score, [r as u8, g as u8, b as u8]));
        }
    }
    let (deep_blue_hex, deep_blue_count) = match best_line {
        Some((max_col, _, rgb_arr)) => (
            Some(format!("{:02x}{:02x}{:02x}", rgb_arr[0], rgb_arr[1], rgb_arr[2])),
            max_col,
        ),
        None => (None, 0),
    };

    // Per-column density analysis. Walks the region by column and counts
    // pixels matching either bar color or the dark-blue/deep-blue target.
    // The max-density column tells us the bar's actual vertical extent and
    // the target line's actual extent — we use those to derive realistic
    // whitePct / minLineDensity thresholds.
    let cols = width as usize;
    let height_px = (rgb.len() as u32) / width.max(1);
    let mut bar_max_col_count: u32 = 0;
    let mut target_max_col_count: u32 = 0;

    // Parse hex helper (small inline — no need for crate dep).
    let hex_to_rgb = |h: &str| -> [i32; 3] {
        let r = u8::from_str_radix(&h[0..2], 16).unwrap_or(0);
        let g = u8::from_str_radix(&h[2..4], 16).unwrap_or(0);
        let b = u8::from_str_radix(&h[4..6], 16).unwrap_or(0);
        [r as i32, g as i32, b as i32]
    };
    let bar_l = left_bar.as_deref().map(hex_to_rgb);
    let bar_r = right_bar.as_deref().map(hex_to_rgb);
    // Target reference: prefer the deep-blue scan result if available
    // (handles thin fish lines), fall back to the dark-blue dominant.
    let target_ref = deep_blue_hex
        .as_deref()
        .or(target_line_dom.as_deref())
        .map(hex_to_rgb);
    const PIX_TOL: i32 = 6;
    if cols > 0 && height_px > 0 {
        for col in 0..cols {
            let mut bar_count: u32 = 0;
            let mut target_count: u32 = 0;
            for row in 0..height_px as usize {
                let idx = row * cols + col;
                if idx >= rgb.len() {
                    break;
                }
                let (r, g, b) = rgb[idx];
                let r_i = r as i32;
                let g_i = g as i32;
                let b_i = b as i32;
                let matches_bar = [bar_l, bar_r].iter().flatten().any(|c| {
                    (r_i - c[0]).abs() <= PIX_TOL
                        && (g_i - c[1]).abs() <= PIX_TOL
                        && (b_i - c[2]).abs() <= PIX_TOL
                });
                if matches_bar {
                    bar_count += 1;
                }
                if let Some(c) = target_ref {
                    if (r_i - c[0]).abs() <= PIX_TOL
                        && (g_i - c[1]).abs() <= PIX_TOL
                        && (b_i - c[2]).abs() <= PIX_TOL
                    {
                        target_count += 1;
                    }
                }
            }
            if bar_count > bar_max_col_count {
                bar_max_col_count = bar_count;
            }
            if target_count > target_max_col_count {
                target_max_col_count = target_count;
            }
        }
    }

    // Suggest thresholds at 70% of the observed max-density. Caps at 80
    // (Hydra's default — we don't recommend going stricter) and floors
    // at 15 (below that, noise overwhelms signal). If the bar is 30 px in
    // a 70 px region, max ratio is 43% → suggest 30%.
    let suggest_pct = |max_count: u32, total_height: u32| -> Option<u8> {
        if max_count == 0 || total_height == 0 {
            return None;
        }
        let ratio = (max_count as f64 / total_height as f64) * 100.0 * 0.7;
        let v = ratio.round().clamp(15.0, 80.0) as u8;
        Some(v)
    };
    let suggested_white_pct = suggest_pct(bar_max_col_count, height_px);
    let suggested_min_line_density =
        suggest_pct(target_max_col_count, height_px);

    // Compute bar tol that bridges the gap between left and right hex.
    // For two colors L and R that are G units apart per channel (worst
    // case), tol must be ≥ G/2 to ensure the union of their match windows
    // is contiguous. Add a 2-unit margin for anti-aliasing.
    let suggested_bar_tol: Option<u8> = match (left_bar.as_deref(), right_bar.as_deref()) {
        (Some(l), Some(r)) => {
            let lc = hex_to_rgb(l);
            let rc = hex_to_rgb(r);
            let max_gap = (lc[0] - rc[0]).abs()
                .max((lc[1] - rc[1]).abs())
                .max((lc[2] - rc[2]).abs());
            // Minimum 4 (a bit more permissive than Hydra's 3) so even
            // identical hexes get a usable tol; max 24 to avoid matching
            // backgrounds that happen to be near-white.
            let tol = ((max_gap / 2) + 2).clamp(4, 24);
            Some(tol as u8)
        }
        _ => None,
    };

    // Target line tol: the fish indicator is anti-aliased and rendered
    // at slightly different shades depending on game state (lighting,
    // motion). 6-10 captures the full anti-aliased range without bleeding
    // into background. We also widen if the deep-blue line was sparse,
    // since sparse means most pixels are edges (anti-aliased).
    let suggested_target_tol: Option<u8> = if deep_blue_count > 0 {
        let tol = if deep_blue_count < 30 { 10 } else { 6 };
        Some(tol)
    } else if target_line_dom.is_some() {
        Some(6)
    } else {
        None
    };

    Ok(CalibrationResult {
        dominants,
        suggested_left_bar: left_bar,
        suggested_right_bar: right_bar,
        // Prefer the vertical-line-detected color (deep_blue_hex from the
        // central-area scan) over the histogram dominant — the latter
        // matches background broadly and produced the wrong-color pick
        // (606070 matching avatar pixels at x=1180+) in log 1777250611.
        // Only fall back to histogram dominant when the line scan found
        // nothing.
        suggested_target_line: deep_blue_hex.clone().or(target_line_dom),
        suggested_arrow: arrow,
        suggested_fish: fish_dom.or_else(|| deep_blue_hex.clone()),
        has_saturated_blue,
        region_pixels: rgb.len() as u32,
        region_width: width,
        region_height: height_px,
        suggested_white_pct,
        suggested_min_line_density,
        suggested_bar_tol,
        suggested_target_tol,
        bar_max_col_count,
        target_max_col_count,
        deep_blue_hex,
        deep_blue_count,
    })
}

/// Tiny inline base64 encoder so we can return PNG bytes as a data URL
/// without adding a base64 crate dependency. The data-URL approach
/// sidesteps Tauri 2's asset-protocol setup (which requires explicit
/// `assetProtocol` config to load `file://` URLs) and Just Works.
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = if chunk.len() > 1 { chunk[1] } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] } else { 0 };
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Capture the entire screen and return a base64 PNG data URL the
/// overlay can drop straight into an <img src> or canvas.drawImage.
/// Used as the backdrop for the F1 region picker and its magnifier.
#[tauri::command]
fn capture_full_screen_data_url() -> Result<String, String> {
    use image::{ImageBuffer, ImageFormat, RgbImage};
    use std::io::Cursor;

    let monitors = Monitor::all().map_err(|e| e.to_string())?;
    let monitor = monitors.first().ok_or("No monitor")?;
    let w = monitor.width().map_err(|e| e.to_string())?;
    let h = monitor.height().map_err(|e| e.to_string())?;

    #[cfg(target_os = "macos")]
    let buf: Vec<u8> = {
        let cap = cg_capture::capture_logical_rect(0, 0, w, h)?;
        let req_w = w.max(1);
        let dpr = (cap.width / req_w).max(1);
        let mut b: Vec<u8> = Vec::with_capacity((w * h * 3) as usize);
        for py in 0..h {
            for px in 0..w {
                let p = cap.pixel_at(px * dpr, py * dpr);
                b.extend_from_slice(&[p[0], p[1], p[2]]);
            }
        }
        b
    };

    #[cfg(not(target_os = "macos"))]
    let buf: Vec<u8> = {
        let (captured, dpr) = capture_with_dpr()?;
        let mut b: Vec<u8> = Vec::with_capacity((w * h * 3) as usize);
        for py in 0..h as i32 {
            for px in 0..w as i32 {
                let p = captured.get_pixel((px as u32) * dpr, (py as u32) * dpr);
                b.extend_from_slice(&[p[0], p[1], p[2]]);
            }
        }
        b
    };

    let img: RgbImage =
        ImageBuffer::from_raw(w, h, buf).ok_or("freeze image build failed")?;
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|e| format!("PNG encode: {}", e))?;
    Ok(format!("data:image/png;base64,{}", base64_encode(&png)))
}

/// Capture and save the fish_bar region as a PNG for offline color
/// inspection. Used to ground-truth tunable hex values when the in-game
/// rendering of a rod variant doesn't match the defaults.
#[tauri::command]
fn save_fish_bar_snapshot(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<String, String> {
    use image::{ImageBuffer, RgbImage};

    let mut dir = dirs::desktop_dir().ok_or("No desktop dir")?;
    dir.push("fisch-macro-debug");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let out_path = dir.join(format!("fish_bar-{}.png", ts));

    #[cfg(target_os = "macos")]
    {
        let cap = cg_capture::capture_logical_rect(x, y, width, height)?;
        let req_w = width.max(1);
        let dpr = (cap.width / req_w).max(1);
        let mut buf = Vec::with_capacity((width * height * 3) as usize);
        for py in 0..height {
            for px in 0..width {
                let p = cap.pixel_at(px * dpr, py * dpr);
                buf.push(p[0]);
                buf.push(p[1]);
                buf.push(p[2]);
            }
        }
        let img: RgbImage = ImageBuffer::from_raw(width, height, buf)
            .ok_or("snapshot image build failed")?;
        img.save(&out_path)
            .map_err(|e| format!("save snapshot: {}", e))?;
    }

    #[cfg(not(target_os = "macos"))]
    {
        let (captured, dpr) = capture_with_dpr()?;
        let img_w = (captured.width() / dpr) as i32;
        let img_h = (captured.height() / dpr) as i32;
        let xs = x.max(0).min(img_w);
        let ys = y.max(0).min(img_h);
        let xe = (x + width as i32).max(0).min(img_w);
        let ye = (y + height as i32).max(0).min(img_h);
        let rw = (xe - xs) as u32;
        let rh = (ye - ys) as u32;
        if rw == 0 || rh == 0 {
            return Err("Empty region".to_string());
        }
        let mut buf = Vec::with_capacity((rw * rh * 3) as usize);
        for py in ys..ye {
            for px in xs..xe {
                let p = captured.get_pixel((px as u32) * dpr, (py as u32) * dpr);
                buf.push(p[0]);
                buf.push(p[1]);
                buf.push(p[2]);
            }
        }
        let img: RgbImage = ImageBuffer::from_raw(rw, rh, buf)
            .ok_or("snapshot image build failed")?;
        img.save(&out_path)
            .map_err(|e| format!("save snapshot: {}", e))?;
    }

    Ok(out_path.to_string_lossy().to_string())
}


/// SHAKE detector — slides the captured template across the search region
/// and reports the position with the lowest SAD. Returns no match if no
/// template has been captured.
#[tauri::command]
fn detect_shake(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    max_avg_diff: Option<u32>,
    excludes: Option<Vec<Region>>,
) -> Result<ShakeResult, String> {
    let template_guard = template_lock().lock().unwrap();
    let template = match template_guard.as_ref() {
        Some(t) => t,
        None => {
            return Ok(ShakeResult {
                centroid: None,
                score: 0,
                threshold: 0,
                has_template: false,
            });
        }
    };

    let (image, dpr) = capture_with_dpr()?;
    let img_w = (image.width() / dpr) as i32;
    let img_h = (image.height() / dpr) as i32;

    let xs = x.max(0).min(img_w);
    let ys = y.max(0).min(img_h);
    let xe = (x + width as i32).max(0).min(img_w);
    let ye = (y + height as i32).max(0).min(img_h);
    let rw = (xe - xs) as u32;
    let rh = (ye - ys) as u32;

    let exclude_rects: Vec<(i32, i32, i32, i32)> = excludes
        .unwrap_or_default()
        .into_iter()
        .map(|r| (r.x, r.y, r.x + r.width as i32, r.y + r.height as i32))
        .collect();

    let max_diff = max_avg_diff.unwrap_or(40);
    let (centroid, best_score, threshold_score) = analyze_shake_template(
        |px, py| {
            let p = image.get_pixel(px * dpr, py * dpr);
            [p[0], p[1], p[2], p[3]]
        },
        xs,
        ys,
        rw,
        rh,
        template,
        max_diff,
        &exclude_rects,
    );

    Ok(ShakeResult {
        centroid,
        score: best_score,
        threshold: threshold_score,
        has_template: true,
    })
}

#[derive(Serialize, Default)]
struct TickResult {
    shake_click: Option<(i32, i32)>,
    shake_score: u64,        // template SAD at best position
    shake_threshold: u64,    // SAD must be ≤ this to count as match
    shake_has_template: bool,
    player_x: Option<i32>,
    fish_x: Option<i32>,
    target_x: Option<i32>,
    capture_ms: u64,
    // Per-tick detection diagnostics. Names are kept short because they
    // appear on every tick log line.
    fb_white_cols: u32,        // total columns matching player-bar colors
    fb_best_run_len: u32,      // widest contiguous player-bar run
    fb_max_grey_per_col: u32,  // highest target-line match count in any col
    // Top 3 player-bar runs as (start_col_within_region, length). Lets us
    // see when a smaller real run is competing with a wider false positive.
    fb_top_runs: Vec<(u32, u32)>,
    fb_run_count: u32,         // total runs found (any width)
    // Region-wide pixel counts for each tunable color. If totals are 0
    // the color isn't on screen; if they're high but per-column threshold
    // fails, the user needs to lower White%/Density.
    // Tier-2 motion detection. Computed by diffing the current fish_bar
    // frame against the previous tick's frame, then finding the column
    // with the most "moving" pixels (excluding bar columns). Independent
    // of color matching — works even when auto-cal hasn't found anything
    // sensible.
    motion_x: Option<i32>,         // x of column with peak motion
    motion_score: u32,             // # of moving pixels in that column
    motion_total: u32,             // # of moving pixels total in region
    fb_total_left: u32,
    fb_total_right: u32,
    fb_total_target: u32,
    fb_total_arrow: u32,
    // X coord of the highest-scoring target-line column even when it
    // didn't clear the density threshold. Lets us see "we almost had it"
    // cases where the user just needs to widen tol or drop density.
    fb_best_target_x: Option<i32>,
    fb_best_target_score: u32,
    // Time spent in the detection loop (column scan), separate from the
    // screen-grab. Helps identify whether a slow tick is GPU/IO or CPU.
    detect_ms: u64,
}

/// One capture, both detections. Single-shot per tick — JS chains these
/// with setTimeout so a new tick never starts while the previous one is in
/// flight.
///
/// `dump_frame_path`: if set, writes the captured fish_bar region as a PNG
/// to that absolute path. Used by the diagnostic frame-dumper toggle so the
/// user can see what the detector is actually scanning. Skipped silently
/// on write errors (we don't want frame-dump failures to break the loop).
#[tauri::command]
fn tick_macro(
    shake: Option<Region>,
    fish_bar: Option<Region>,
    max_avg_diff: Option<u32>,
    extra_excludes: Option<Vec<Region>>,
    fish_params: Option<FishColorParams>,
    dump_frame_path: Option<String>,
) -> Result<TickResult, String> {
    let fp = fish_params.unwrap_or_default();
    let t0 = std::time::Instant::now();

    // On macOS, capture ONLY the union of (shake, fish_bar) rather than the
    // full screen. CGDisplayCreateImageForRect is much faster for small
    // rects than a full-screen xcap capture (~50ms vs ~500ms on retina).
    #[cfg(target_os = "macos")]
    let (cg_cap, cap_origin_x, cap_origin_y, dpr) = {
        let mut union: Option<(i32, i32, i32, i32)> = None;
        let mut extend = |x: i32, y: i32, w: u32, h: u32| {
            let x2 = x + w as i32;
            let y2 = y + h as i32;
            union = Some(match union {
                None => (x, y, x2, y2),
                Some((a, b, c, d)) => (a.min(x), b.min(y), c.max(x2), d.max(y2)),
            });
        };
        if let Some(s) = shake.as_ref() {
            extend(s.x, s.y, s.width, s.height);
        }
        if let Some(f) = fish_bar.as_ref() {
            extend(f.x, f.y, f.width, f.height);
        }
        match union {
            Some((x1, y1, x2, y2)) => {
                let cap = cg_capture::capture_logical_rect(
                    x1,
                    y1,
                    (x2 - x1) as u32,
                    (y2 - y1) as u32,
                )?;
                let req_w = (x2 - x1) as u32;
                let dpr = if req_w == 0 { 1 } else { (cap.width / req_w).max(1) };
                (Some(cap), x1, y1, dpr)
            }
            None => (None, 0, 0, 1),
        }
    };

    // Fallback path for non-macOS: full-screen xcap capture.
    #[cfg(not(target_os = "macos"))]
    let (image, dpr) = capture_with_dpr()?;

    let capture_ms = t0.elapsed().as_millis() as u64;

    #[cfg(target_os = "macos")]
    let (img_w, img_h) = {
        let monitors = Monitor::all().map_err(|e| e.to_string())?;
        let monitor = monitors.first().ok_or("No monitor found")?;
        let mw = monitor.width().map_err(|e| e.to_string())? as i32;
        let mh = monitor.height().map_err(|e| e.to_string())? as i32;
        (mw, mh)
    };

    #[cfg(not(target_os = "macos"))]
    let img_w = (image.width() / dpr) as i32;
    #[cfg(not(target_os = "macos"))]
    let img_h = (image.height() / dpr) as i32;

    let mut out = TickResult {
        capture_ms,
        ..Default::default()
    };

    if let Some(sh) = shake {
        let xs = sh.x.max(0).min(img_w);
        let ys = sh.y.max(0).min(img_h);
        let xe = (sh.x + sh.width as i32).max(0).min(img_w);
        let ye = (sh.y + sh.height as i32).max(0).min(img_h);
        let rw = (xe - xs).max(0) as u32;
        let rh = (ye - ys).max(0) as u32;

        // Build exclude list: fish-bar (auto) + anything JS passes in
        // (typically the macro window itself, so it doesn't block detection).
        let mut exclude_rects: Vec<(i32, i32, i32, i32)> = Vec::new();
        if let Some(fb) = fish_bar.as_ref() {
            exclude_rects.push((
                fb.x,
                fb.y,
                fb.x + fb.width as i32,
                fb.y + fb.height as i32,
            ));
        }
        if let Some(extras) = extra_excludes.as_ref() {
            for r in extras {
                exclude_rects.push((
                    r.x,
                    r.y,
                    r.x + r.width as i32,
                    r.y + r.height as i32,
                ));
            }
        }

        // Template matching — replaces structural / baseline-diff entirely.
        let template_guard = template_lock().lock().unwrap();
        if let Some(template) = template_guard.as_ref() {
            let max_diff = max_avg_diff.unwrap_or(40);
            #[cfg(target_os = "macos")]
            let pixel_at = |px: u32, py: u32| -> [u8; 4] {
                let cap = cg_cap.as_ref().unwrap();
                let rel_x = (px as i32 - cap_origin_x) as u32 * dpr;
                let rel_y = (py as i32 - cap_origin_y) as u32 * dpr;
                cap.pixel_at(rel_x, rel_y)
            };
            #[cfg(not(target_os = "macos"))]
            let pixel_at = |px: u32, py: u32| -> [u8; 4] {
                let p = image.get_pixel(px * dpr, py * dpr);
                [p[0], p[1], p[2], p[3]]
            };
            let (centroid, best_score, threshold_score) = analyze_shake_template(
                pixel_at,
                xs,
                ys,
                rw,
                rh,
                template,
                max_diff,
                &exclude_rects,
            );
            out.shake_click = centroid;
            out.shake_score = best_score;
            out.shake_threshold = threshold_score;
            out.shake_has_template = true;
        } else {
            out.shake_has_template = false;
        }
    }

    if let Some(fb) = fish_bar {
        let xs = fb.x.max(0).min(img_w);
        let ys = fb.y.max(0).min(img_h);
        let xe = (fb.x + fb.width as i32).max(0).min(img_w);
        let ye = (fb.y + fb.height as i32).max(0).min(img_h);
        if xe > xs && ye > ys {
            // Hydra "Color" track-style detection. Each element's hex/tol is
            // user-tunable from the Fish tab — see FishColorParams. We test
            // each pixel against the user-configured RGB targets and count
            // matches per column.
            let player_left: [i32; 3] = [
                fp.left_bar[0] as i32,
                fp.left_bar[1] as i32,
                fp.left_bar[2] as i32,
            ];
            let player_right: [i32; 3] = [
                fp.right_bar[0] as i32,
                fp.right_bar[1] as i32,
                fp.right_bar[2] as i32,
            ];
            let target_line: [i32; 3] = [
                fp.target_line[0] as i32,
                fp.target_line[1] as i32,
                fp.target_line[2] as i32,
            ];
            let left_tol = fp.left_tol as i32;
            let right_tol = fp.right_tol as i32;
            let target_tol = fp.target_tol as i32;
            let col_match_ratio = (fp.white_pct as f64 / 100.0).clamp(0.05, 1.0);
            let min_bar_width = fp.min_line_count.max(1) as usize;

            let cols = (xe - xs) as usize;
            let height = (ye - ys) as u32;
            let min_match_per_col =
                ((height as f64) * col_match_ratio).round().max(1.0) as u32;

            let mut col_is_player = vec![false; cols];
            let mut col_target_count: Vec<u32> = vec![0; cols];
            // Region-wide pixel counts per color. Independent of the
            // per-column threshold so we can tell "color is totally
            // absent" from "color is there but not dense enough."
            let mut total_left: u32 = 0;
            let mut total_right: u32 = 0;
            let mut total_target: u32 = 0;
            let mut total_arrow: u32 = 0;
            let arrow_color: [i32; 3] = [
                fp.arrow[0] as i32,
                fp.arrow[1] as i32,
                fp.arrow[2] as i32,
            ];
            let arrow_tol = fp.arrow_tol as i32;

            let detect_t0 = std::time::Instant::now();

            for (i, px) in (xs..xe).enumerate() {
                let mut player_count: u32 = 0;
                let mut target_count: u32 = 0;
                for py in ys..ye {
                    #[cfg(target_os = "macos")]
                    let p = {
                        let cap = cg_cap.as_ref().unwrap();
                        let rel_x = (px - cap_origin_x) as u32 * dpr;
                        let rel_y = (py - cap_origin_y) as u32 * dpr;
                        cap.pixel_at(rel_x, rel_y)
                    };
                    #[cfg(not(target_os = "macos"))]
                    let p = image.get_pixel((px as u32) * dpr, (py as u32) * dpr);
                    let r = p[0] as i32;
                    let g = p[1] as i32;
                    let b = p[2] as i32;
                    let matches_left = (r - player_left[0]).abs() <= left_tol
                        && (g - player_left[1]).abs() <= left_tol
                        && (b - player_left[2]).abs() <= left_tol;
                    let matches_right = (r - player_right[0]).abs() <= right_tol
                        && (g - player_right[1]).abs() <= right_tol
                        && (b - player_right[2]).abs() <= right_tol;
                    if matches_left {
                        total_left += 1;
                    }
                    if matches_right {
                        total_right += 1;
                    }
                    if matches_left || matches_right {
                        player_count += 1;
                        continue;
                    }
                    let matches_target = (r - target_line[0]).abs() <= target_tol
                        && (g - target_line[1]).abs() <= target_tol
                        && (b - target_line[2]).abs() <= target_tol;
                    if matches_target {
                        target_count += 1;
                        total_target += 1;
                        continue;
                    }
                    let matches_arrow = (r - arrow_color[0]).abs() <= arrow_tol
                        && (g - arrow_color[1]).abs() <= arrow_tol
                        && (b - arrow_color[2]).abs() <= arrow_tol;
                    if matches_arrow {
                        total_arrow += 1;
                    }
                }
                col_is_player[i] = player_count >= min_match_per_col;
                col_target_count[i] = target_count;
            }

            // Enumerate ALL contiguous player-bar runs. Hydra merges runs
            // separated by ≤"Merge Distance" px gaps so we do the same to
            // absorb single-column dropouts from anti-aliasing.
            let merge_distance = fp.merge_distance as usize;
            let mut runs: Vec<(usize, usize)> = Vec::new();
            {
                let mut i = 0;
                while i < cols {
                    if col_is_player[i] {
                        let start = i;
                        let mut end = i;
                        while end < cols {
                            if col_is_player[end] {
                                end += 1;
                            } else {
                                // Look ahead up to merge_distance for another
                                // matching column; if we find one, bridge the
                                // gap and keep extending.
                                let mut bridged = false;
                                for la in 1..=merge_distance.max(1) {
                                    if end + la < cols && col_is_player[end + la] {
                                        end += la;
                                        bridged = true;
                                        break;
                                    }
                                }
                                if !bridged {
                                    break;
                                }
                            }
                        }
                        runs.push((start, end - start));
                        i = end + 1;
                    } else {
                        i += 1;
                    }
                }
            }

            // Pick the longest run of valid width as the player bar. With
            // the exact-color match the only thing at this column-density
            // that's left/right-bar colored should be the bar itself —
            // there's no MAX cap because rod variants make the bar wider/
            // narrower and the strict color match handles disambiguation.
            let player_run = runs
                .iter()
                .copied()
                .filter(|(_, len)| *len >= min_bar_width)
                .max_by_key(|(_, len)| *len);
            let player_x_opt = player_run
                .map(|(start, len)| xs + ((start + len / 2) as i32));

            // Diagnostics for the debug log.
            let total_player_cols =
                col_is_player.iter().filter(|&&v| v).count() as u32;
            let widest_run_len = runs
                .iter()
                .map(|(_, l)| *l)
                .max()
                .unwrap_or(0) as u32;
            // Top 3 player-bar runs (by length, descending). Surfaces the
            // case where a wider false-positive is competing with the real
            // bar — without this we can only see the winner.
            let mut sorted_runs = runs.clone();
            sorted_runs.sort_by_key(|(_, l)| std::cmp::Reverse(*l));
            let top_runs: Vec<(u32, u32)> = sorted_runs
                .iter()
                .take(3)
                .map(|(s, l)| (*s as u32, *l as u32))
                .collect();
            // Best target-line column anywhere in the region — even if it
            // didn't clear the density threshold. Helps the user see "we
            // almost had it" cases vs "the color is completely absent."
            let (best_target_col, best_target_count) = col_target_count
                .iter()
                .copied()
                .enumerate()
                .max_by_key(|&(_, v)| v)
                .map(|(i, v)| (i, v))
                .unwrap_or((0, 0));
            out.fb_white_cols = total_player_cols;
            out.fb_best_run_len = widest_run_len;
            out.fb_max_grey_per_col = best_target_count;
            out.fb_run_count = runs.len() as u32;
            out.fb_top_runs = top_runs;
            out.fb_total_left = total_left;
            out.fb_total_right = total_right;
            out.fb_total_target = total_target;
            out.fb_total_arrow = total_arrow;
            out.fb_best_target_score = best_target_count;
            out.fb_best_target_x = if best_target_count > 0 {
                Some(xs + best_target_col as i32)
            } else {
                None
            };

            // Fish detection: pick the column with the most target-line
            // pixels. Exclude:
            //   1. Edge margins (UI brackets / frame artifacts)
            //   2. Columns inside the player bar (the bar occludes the line)
            // Threshold uses the user-configured "Min Line Density".
            let edge_margin_cols = (fp.edge_touch as usize).max(1);
            let line_density_ratio =
                (fp.min_line_density as f64 / 100.0).clamp(0.05, 1.0);
            let min_target_for_fish =
                ((height as f64) * line_density_ratio).round().max(2.0) as u32;
            let (player_excl_start, player_excl_len) = match player_run {
                Some((s, l)) => (s, l),
                None => (0, 0),
            };
            let has_player_excl = player_excl_len > 0;
            let excl_lo = if has_player_excl {
                player_excl_start
            } else {
                0
            };
            let excl_hi = if has_player_excl {
                player_excl_start + player_excl_len
            } else {
                0
            };

            let mut best_fish_score: u32 = 0;
            let mut best_fish_x: i32 = xs;
            for i in edge_margin_cols..(cols.saturating_sub(edge_margin_cols)) {
                if has_player_excl && i >= excl_lo && i < excl_hi {
                    continue;
                }
                if col_target_count[i] > best_fish_score {
                    best_fish_score = col_target_count[i];
                    best_fish_x = xs + i as i32;
                }
            }
            let fish_x_opt = if best_fish_score >= min_target_for_fish {
                Some(best_fish_x)
            } else {
                None
            };

            // ---- Tier-2 temporal motion detection ----
            // Build the current frame's RGB buffer for the fish_bar region
            // and compare with the previous tick's buffer. Pixels that
            // changed beyond a small threshold are "moving". The bar's own
            // columns are excluded so motion *outside* the bar is what we
            // pick — that's the fish (or fish indicator). This works even
            // when color matching can't tell the fish from end-cap markers
            // because static UI doesn't move at all.
            let mut cur_buf: Vec<u8> = Vec::with_capacity(cols * height as usize * 3);
            for py in ys..ye {
                for px in xs..xe {
                    #[cfg(target_os = "macos")]
                    let p = {
                        let cap = cg_cap.as_ref().unwrap();
                        let rel_x = (px - cap_origin_x) as u32 * dpr;
                        let rel_y = (py - cap_origin_y) as u32 * dpr;
                        cap.pixel_at(rel_x, rel_y)
                    };
                    #[cfg(not(target_os = "macos"))]
                    let p = image.get_pixel((px as u32) * dpr, (py as u32) * dpr);
                    cur_buf.extend_from_slice(&[p[0], p[1], p[2]]);
                }
            }

            let mut motion_x_opt: Option<i32> = None;
            let mut motion_peak: u32 = 0;
            let mut motion_total: u32 = 0;
            const MOTION_PIX_THRESHOLD: i32 = 30;
            if let Ok(mut prev_lock) = prev_fish_frame().lock() {
                if let Some((prev_w, prev_h, prev_buf)) = prev_lock.as_ref() {
                    if *prev_w == cols as u32 && *prev_h == height
                        && prev_buf.len() == cur_buf.len()
                    {
                        let mut col_motion = vec![0u32; cols];
                        for row in 0..height as usize {
                            for col in 0..cols {
                                let i = (row * cols + col) * 3;
                                let dr = (cur_buf[i] as i32 - prev_buf[i] as i32).abs();
                                let dg =
                                    (cur_buf[i + 1] as i32 - prev_buf[i + 1] as i32).abs();
                                let db =
                                    (cur_buf[i + 2] as i32 - prev_buf[i + 2] as i32).abs();
                                if dr + dg + db > MOTION_PIX_THRESHOLD {
                                    col_motion[col] += 1;
                                    motion_total += 1;
                                }
                            }
                        }
                        // Zero out columns inside the player bar so the bar's
                        // own anti-aliased edge motion doesn't dominate.
                        if has_player_excl {
                            for col in excl_lo..excl_hi.min(cols) {
                                col_motion[col] = 0;
                            }
                        }
                        // Also zero edges (frame artifacts on capture boundary).
                        let edge = (cols / 24).max(2);
                        for col in 0..edge.min(cols) {
                            col_motion[col] = 0;
                        }
                        for col in cols.saturating_sub(edge)..cols {
                            col_motion[col] = 0;
                        }
                        // Centroid (center-of-mass) of motion across columns,
                        // not just argmax. Reason: with a thin fish_bar
                        // region (40 px tall), score saturates at 40 in any
                        // fully-motion column — and many columns saturate
                        // simultaneously when a real moving feature is wider
                        // than 1 px. argmax then picks an arbitrary saturated
                        // column (often the leftmost or rightmost edge of the
                        // motion blob), which causes the target to flip
                        // between extremes tick-to-tick. Centroid gives the
                        // weighted-average column = stable middle of the
                        // motion blob, even when many columns are saturated.
                        let mut weight_sum: f64 = 0.0;
                        let mut weight_count: f64 = 0.0;
                        for (col, &m) in col_motion.iter().enumerate() {
                            if m >= 4 {
                                weight_sum += (col as f64) * (m as f64);
                                weight_count += m as f64;
                                if m > motion_peak {
                                    motion_peak = m;
                                }
                            }
                        }
                        if weight_count > 0.0 {
                            let centroid_col = (weight_sum / weight_count).round() as i32;
                            motion_x_opt = Some(xs + centroid_col);
                        }
                    }
                }
                *prev_lock = Some((cols as u32, height, cur_buf));
            }

            out.motion_x = motion_x_opt;
            out.motion_score = motion_peak;
            out.motion_total = motion_total;

            out.player_x = player_x_opt;
            out.fish_x = fish_x_opt;
            // Target is the fish line when present; otherwise the bar center
            // as a stable fallback (the controller will hold the player at
            // center until the line reappears).
            out.target_x = match fish_x_opt {
                Some(fx) => Some(fx),
                None if player_x_opt.is_some() => {
                    Some(fb.x + (fb.width as i32) / 2)
                }
                None => None,
            };
            out.detect_ms = detect_t0.elapsed().as_millis() as u64;

            // Diagnostic frame dump. Writes the exact bytes the detector saw
            // as a PNG so the user can verify the bar is in-region, the
            // colors look right, and the calibration matches reality. Only
            // runs when JS supplies a path. Failures are swallowed — disk
            // hiccups shouldn't kill the macro loop.
            if let Some(path) = &dump_frame_path {
                let dw = (xe - xs) as u32;
                let dh = (ye - ys) as u32;
                if dw > 0 && dh > 0 {
                    let mut rgb_bytes: Vec<u8> =
                        Vec::with_capacity((dw as usize) * (dh as usize) * 3);
                    for py in ys..ye {
                        for px in xs..xe {
                            #[cfg(target_os = "macos")]
                            let p = {
                                let cap = cg_cap.as_ref().unwrap();
                                let rel_x = (px - cap_origin_x) as u32 * dpr;
                                let rel_y = (py - cap_origin_y) as u32 * dpr;
                                cap.pixel_at(rel_x, rel_y)
                            };
                            #[cfg(not(target_os = "macos"))]
                            let p = image.get_pixel((px as u32) * dpr, (py as u32) * dpr);
                            rgb_bytes.push(p[0]);
                            rgb_bytes.push(p[1]);
                            rgb_bytes.push(p[2]);
                        }
                    }
                    if let Some(img) = ::image::RgbImage::from_raw(dw, dh, rgb_bytes) {
                        if let Some(parent) = std::path::Path::new(path).parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = img.save(path);
                    }
                }
            }
        }
    }

    Ok(out)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Restore the persisted SHAKE template (if any) before any commands run,
    // so `has_shake_template` reports correctly on first paint.
    if let Some(t) = load_template_from_disk() {
        *template_lock().lock().unwrap() = Some(t);
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .invoke_handler(tauri::generate_handler![
            click_at,
            mouse_move,
            mouse_down,
            mouse_up,
            start_enter_spam,
            stop_enter_spam,
            start_m1_rapid_click,
            stop_m1_rapid_click,
            start_m1_pwm,
            stop_m1_pwm,
            set_m1_pwm_duty,
            debug_log_start,
            debug_log_stop,
            debug_log_append,
            get_frames_run_dir,
            find_player_x,
            detect_shake,
            capture_shake_template,
            clear_shake_template,
            has_shake_template,
            save_shake_template_image,
            debug_save_cg_full,
            debug_save_full_with_marker,
            save_shake_snapshot,
            save_fish_bar_snapshot,
            capture_full_screen_data_url,
            set_target_window,
            list_windows,
            calibrate_fish_colors,
            tick_macro,
            load_regions,
            save_regions,
            get_screen_size
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
