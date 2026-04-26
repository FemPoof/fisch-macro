#[cfg(not(target_os = "macos"))]
use enigo::{Button, Coordinate, Direction, Enigo, Mouse, Settings};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use xcap::Monitor;

/// Baseline snapshot of the shake region (RGB), captured when no SHAKE button
/// is visible. Detection then looks for pixels that DIFFER from baseline —
/// scenery is static, the button popping in is a huge change.
///
/// `excludes_at_capture` records rects that were on screen at baseline time
/// (e.g. the macro window). At detection time these are still skipped even
/// if they've moved, so the move itself doesn't poison the diff.
struct ShakeBaseline {
    region_x: i32,
    region_y: i32,
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

#[cfg(target_os = "macos")]
mod cg_capture {
    use std::ffi::c_void;
    use xcap::Monitor;

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
        /// Returns [R, G, B, A].
        #[inline]
        pub fn pixel_at(&self, px: u32, py: u32) -> [u8; 4] {
            let abs_x = self.offset_x_phys + px;
            let abs_y = self.offset_y_phys + py;
            let idx = (abs_y as usize) * self.bytes_per_row + (abs_x as usize) * 4;
            // macOS stores pixels as BGRA; convert to RGBA on read.
            let b = self.data[idx];
            let g = self.data[idx + 1];
            let r = self.data[idx + 2];
            let a = self.data[idx + 3];
            [r, g, b, a]
        }
    }

    /// Capture the rectangle (x, y, w, h) in LOGICAL pixels. The returned
    /// view is in PHYSICAL pixels (Retina = 2× the logical size).
    ///
    /// IMPLEMENTATION NOTE: We always capture the full display via
    /// `CGDisplayCreateImageForRect(display, full_bounds)` and crop in-process.
    /// Calling CG with a non-zero origin returned wrong pixels on this user's
    /// macOS 14+ HiDPI-scaled setup (likely a deprecation-era bug — the API is
    /// being phased out in favor of ScreenCaptureKit). Full-screen capture is
    /// reliable, costs ~one extra 30MB memcpy, and is well within budget at
    /// our 500ms tick gap.
    pub fn capture_logical_rect(x: i32, y: i32, w: u32, h: u32) -> Result<PartialCapture, String> {
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
}

#[derive(Serialize)]
struct AverageColor {
    r: u8,
    g: u8,
    b: u8,
    pixels: u32,
}

#[derive(Serialize, Deserialize, Clone)]
struct Region {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
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

#[tauri::command]
fn capture_region(x: i32, y: i32, width: u32, height: u32) -> Result<AverageColor, String> {
    let (image, dpr) = capture_with_dpr()?;
    let img_w = (image.width() / dpr) as i32;
    let img_h = (image.height() / dpr) as i32;

    let x_start = x.max(0).min(img_w);
    let y_start = y.max(0).min(img_h);
    let x_end = (x + width as i32).max(0).min(img_w);
    let y_end = (y + height as i32).max(0).min(img_h);

    let mut r_sum: u64 = 0;
    let mut g_sum: u64 = 0;
    let mut b_sum: u64 = 0;
    let mut count: u64 = 0;

    for py in y_start..y_end {
        for px in x_start..x_end {
            let pixel = image.get_pixel((px as u32) * dpr, (py as u32) * dpr);
            r_sum += pixel[0] as u64;
            g_sum += pixel[1] as u64;
            b_sum += pixel[2] as u64;
            count += 1;
        }
    }

    if count == 0 {
        return Err("No pixels in region (check coordinates vs screen size)".to_string());
    }

    Ok(AverageColor {
        r: (r_sum / count) as u8,
        g: (g_sum / count) as u8,
        b: (b_sum / count) as u8,
        pixels: count as u32,
    })
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

/// Capture a baseline snapshot of the shake region. Call when the SHAKE
/// button is NOT visible — detection compares future captures to this.
/// `excludes` (e.g. the macro window's current position) are stored along
/// with the baseline so they're masked even if the window later moves.
#[tauri::command]
fn capture_shake_baseline(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    excludes: Option<Vec<Region>>,
) -> Result<u32, String> {
    let (image, dpr) = capture_with_dpr()?;
    let img_w = (image.width() / dpr) as i32;
    let img_h = (image.height() / dpr) as i32;
    let xs = x.max(0).min(img_w);
    let ys = y.max(0).min(img_h);
    let xe = (x + width as i32).max(0).min(img_w);
    let ye = (y + height as i32).max(0).min(img_h);
    let rw = (xe - xs).max(0) as u32;
    let rh = (ye - ys).max(0) as u32;

    if rw == 0 || rh == 0 {
        return Err("Empty region".to_string());
    }

    let mut rgb = Vec::with_capacity((rw as usize) * (rh as usize) * 3);
    for py in ys..ye {
        for px in xs..xe {
            let p = image.get_pixel((px as u32) * dpr, (py as u32) * dpr);
            rgb.push(p[0]);
            rgb.push(p[1]);
            rgb.push(p[2]);
        }
    }

    let excludes_at_capture: Vec<(i32, i32, i32, i32)> = excludes
        .unwrap_or_default()
        .into_iter()
        .map(|r| (r.x, r.y, r.x + r.width as i32, r.y + r.height as i32))
        .collect();

    *baseline_lock().lock().unwrap() = Some(ShakeBaseline {
        region_x: xs,
        region_y: ys,
        width: rw,
        height: rh,
        rgb,
        excludes_at_capture,
    });
    Ok(rw * rh)
}

#[tauri::command]
fn clear_shake_baseline() -> Result<(), String> {
    *baseline_lock().lock().unwrap() = None;
    Ok(())
}

#[tauri::command]
fn has_shake_baseline() -> bool {
    baseline_lock().lock().unwrap().is_some()
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
            for ty in 0..sth {
                for tx in 0..stw {
                    let cx = sx + tx * s;
                    let cy = sy + ty * s;
                    let avg = cell_avg(cx, cy);
                    p_avg[(ty * stw + tx) as usize] = avg;
                    p_sum += avg;
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

/// Returns true if the captured baseline matches the given region's
/// origin + dimensions (within ±2 px tolerance for rounding).
#[allow(dead_code)] // legacy — used by analyze_shake_diff which is no longer wired up
fn baseline_matches(b: &ShakeBaseline, xs: i32, ys: i32, rw: u32, rh: u32) -> bool {
    (b.region_x - xs).abs() <= 2
        && (b.region_y - ys).abs() <= 2
        && (b.width as i32 - rw as i32).abs() <= 2
        && (b.height as i32 - rh as i32).abs() <= 2
}

/// Baseline-diff + sliding-window densest-cluster detector. For each pixel,
/// compute |R_now - R_base| + |G| + |B|. Pixels exceeding `diff_threshold`
/// are "changed". Then slide a window across the change-mask to find the
/// densest cluster — the SHAKE button creates a tight cluster, water shimmer
/// is scattered.
///
/// Returns (centroid_within_densest_window, total_changed, densest_window_count).
#[allow(dead_code)] // retired in favor of analyze_shake_structure (lighting-invariant)
fn analyze_shake_diff<F: Fn(u32, u32) -> [u8; 4]>(
    pixel_at: F,
    baseline: &ShakeBaseline,
    xs: i32,
    ys: i32,
    rw: u32,
    rh: u32,
    diff_threshold: u32,
    min_window_pixels: u32,
    excludes: &[(i32, i32, i32, i32)], // list of (x1, y1, x2, y2) absolute
) -> (Option<(i32, i32)>, u32, u32) {
    if !baseline_matches(baseline, xs, ys, rw, rh) || rw == 0 || rh == 0 {
        return (None, 0, 0);
    }
    let w = rw as usize;
    let h = rh as usize;

    // Build "changed" mask + total. Pixels in any exclude rect (fish-bar,
    // macro window, etc.) are skipped so they don't poison the search.
    let mut mask = vec![0u32; w * h];
    let mut total_changed: u32 = 0;
    for py in 0..rh {
        for px in 0..rw {
            let abs_x = xs + px as i32;
            let abs_y = ys + py as i32;
            if in_any_exclude(abs_x, abs_y, excludes) {
                continue;
            }
            let p = pixel_at(abs_x as u32, abs_y as u32);
            let idx = (py as usize * w + px as usize) * 3;
            let br = baseline.rgb[idx] as i32;
            let bg = baseline.rgb[idx + 1] as i32;
            let bb = baseline.rgb[idx + 2] as i32;
            let dr = (p[0] as i32 - br).unsigned_abs();
            let dg = (p[1] as i32 - bg).unsigned_abs();
            let db = (p[2] as i32 - bb).unsigned_abs();
            if dr + dg + db >= diff_threshold {
                mask[py as usize * w + px as usize] = 1;
                total_changed += 1;
            }
        }
    }

    // Integral image
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

    let win = ((rw.min(rh)) / 3).clamp(60, 400) as usize;
    if win == 0 || win > w || win > h {
        return (None, total_changed, 0);
    }

    let mut best_count: u32 = 0;
    let mut best_x: usize = 0;
    let mut best_y: usize = 0;
    let max_y = h - win;
    let max_x = w - win;
    for py in 0..=max_y {
        for px in 0..=max_x {
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

    // Compute centroid even when below threshold so the UI can surface the
    // strongest cluster seen (useful for tuning). Caller checks best_count
    // against min_window_pixels to decide whether to act.
    let _ = min_window_pixels; // documented in caller; not used here
    if best_count == 0 {
        return (None, total_changed, best_count);
    }
    let mut sx: u64 = 0;
    let mut sy: u64 = 0;
    let mut cnt: u64 = 0;
    for py in best_y..(best_y + win) {
        for px in best_x..(best_x + win) {
            if mask[py * w + px] == 1 {
                sx += xs as u64 + px as u64;
                sy += ys as u64 + py as u64;
                cnt += 1;
            }
        }
    }
    if cnt == 0 {
        return (None, total_changed, best_count);
    }
    (
        Some(((sx / cnt) as i32, (sy / cnt) as i32)),
        total_changed,
        best_count,
    )
}

/// Structural SHAKE-button detector. Finds a window containing BOTH:
///   - A dense cluster of very dark pixels (the button body)
///   - At least some very bright pixels in the same window (the white
///     "SHAKE" text inside)
///
/// This is invariant to ambient lighting — the SHAKE button is always
/// "near-black with near-white text", regardless of day/night. So unlike
/// baseline-diff this doesn't fall apart when the game's day/night cycle
/// shifts the whole scene.
///
/// Returns (centroid_of_dark_pixels_in_best_window, total_dark, total_bright,
/// best_dark_in_window, best_bright_in_window).
#[allow(dead_code)] // retired in favor of analyze_shake_template
fn analyze_shake_structure<F: Fn(u32, u32) -> [u8; 4]>(
    pixel_at: F,
    region_x: i32,
    region_y: i32,
    region_w: u32,
    region_h: u32,
    dark_threshold: u8,    // each channel must be < this for "dark"
    bright_threshold: u8,  // each channel must be > this for "bright" (= near-white)
    min_dark_in_window: u32,
    min_bright_in_window: u32,
    excludes: &[(i32, i32, i32, i32)],
) -> (Option<(i32, i32)>, u32, u32, u32, u32) {
    if region_w == 0 || region_h == 0 {
        return (None, 0, 0, 0, 0);
    }
    let w = region_w as usize;
    let h = region_h as usize;

    let mut dark_mask = vec![0u32; w * h];
    let mut bright_mask = vec![0u32; w * h];
    let mut total_dark: u32 = 0;
    let mut total_bright: u32 = 0;

    // Per-channel checks: a pixel only counts as dark/bright if ALL THREE
    // channels are below dark_threshold / above bright_threshold. This rules
    // out colored extremes — dark blue sky (R=20, G=30, B=80) won't pass
    // dark; yellow level labels (R=255, G=220, B=80) won't pass bright. Only
    // near-grayscale pixels qualify, which is what the SHAKE button is.
    for py in 0..region_h {
        for px in 0..region_w {
            let abs_x = region_x + px as i32;
            let abs_y = region_y + py as i32;
            if in_any_exclude(abs_x, abs_y, excludes) {
                continue;
            }
            let p = pixel_at(abs_x as u32, abs_y as u32);
            let idx = py as usize * w + px as usize;
            if p[0] < dark_threshold && p[1] < dark_threshold && p[2] < dark_threshold {
                dark_mask[idx] = 1;
                total_dark += 1;
            }
            if p[0] > bright_threshold && p[1] > bright_threshold && p[2] > bright_threshold {
                bright_mask[idx] = 1;
                total_bright += 1;
            }
        }
    }

    // Two integral images
    let iw = w + 1;
    let mut dark_ii = vec![0u32; iw * (h + 1)];
    let mut bright_ii = vec![0u32; iw * (h + 1)];
    for py in 1..=h {
        for px in 1..=w {
            dark_ii[py * iw + px] = dark_mask[(py - 1) * w + (px - 1)]
                + dark_ii[py * iw + (px - 1)]
                + dark_ii[(py - 1) * iw + px]
                - dark_ii[(py - 1) * iw + (px - 1)];
            bright_ii[py * iw + px] = bright_mask[(py - 1) * w + (px - 1)]
                + bright_ii[py * iw + (px - 1)]
                + bright_ii[(py - 1) * iw + px]
                - bright_ii[(py - 1) * iw + (px - 1)];
        }
    }

    let win = ((region_w.min(region_h)) / 3).clamp(60, 400) as usize;
    if win == 0 || win > w || win > h {
        return (None, total_dark, total_bright, 0, 0);
    }

    let mut best_dark: u32 = 0;
    let mut best_bright: u32 = 0;
    let mut best_x: usize = 0;
    let mut best_y: usize = 0;
    let max_y = h - win;
    let max_x = w - win;

    // Reject windows where bright pixels are too widespread — clouds, the
    // lantern interior, lit signs, etc. Cap fixed at 1200 because the SHAKE
    // text is small (a single word) regardless of region/window size.
    let max_bright_in_window: u32 = 1200;

    for py in 0..=max_y {
        for px in 0..=max_x {
            // Dark count via integral image
            let a = dark_ii[(py + win) * iw + (px + win)];
            let b = dark_ii[py * iw + (px + win)];
            let c = dark_ii[(py + win) * iw + px];
            let d = dark_ii[py * iw + px];
            let dark_count = (a + d) - (b + c);

            // Quick reject if not enough dark
            if dark_count < min_dark_in_window {
                continue;
            }

            // Bright count via integral image
            let ab = bright_ii[(py + win) * iw + (px + win)];
            let bb = bright_ii[py * iw + (px + win)];
            let cb = bright_ii[(py + win) * iw + px];
            let db = bright_ii[py * iw + px];
            let bright_count = (ab + db) - (bb + cb);

            // Reject too-few-bright (no text cluster) and too-much-bright
            // (lantern, glow, big bright UI). Only sparse-bright-in-dark
            // qualifies — that's the SHAKE button's structure.
            if bright_count < min_bright_in_window || bright_count > max_bright_in_window {
                continue;
            }

            // Score by BRIGHT count among qualifying — favors windows with
            // a meaningful text cluster over pure-dark windows that just
            // happen to graze a few stray bright pixels.
            if bright_count > best_bright {
                best_dark = dark_count;
                best_bright = bright_count;
                best_x = px;
                best_y = py;
            }
        }
    }

    if best_dark == 0 {
        return (None, total_dark, total_bright, 0, 0);
    }

    // Centroid of dark pixels in the qualifying best window
    let mut sx: u64 = 0;
    let mut sy: u64 = 0;
    let mut cnt: u64 = 0;
    for py in best_y..(best_y + win) {
        for px in best_x..(best_x + win) {
            if dark_mask[py * w + px] == 1 {
                sx += region_x as u64 + px as u64;
                sy += region_y as u64 + py as u64;
                cnt += 1;
            }
        }
    }
    if cnt == 0 {
        return (None, total_dark, total_bright, best_dark, best_bright);
    }
    (
        Some(((sx / cnt) as i32, (sy / cnt) as i32)),
        total_dark,
        total_bright,
        best_dark,
        best_bright,
    )
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
}

/// One capture, both detections. Single-shot per tick — JS chains these
/// with setTimeout so a new tick never starts while the previous one is in
/// flight.
#[tauri::command]
fn tick_macro(
    shake: Option<Region>,
    fish_bar: Option<Region>,
    max_avg_diff: Option<u32>,
    extra_excludes: Option<Vec<Region>>,
) -> Result<TickResult, String> {
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
            // For each column we accumulate two scores in one pass:
            //   white_score  → high for bright grayscale pixels (the player
            //                  marker, which is rendered as a white bar).
            //   color_score  → high for bright saturated pixels (the fish
            //                  icon, which is rendered with vivid colors).
            // Player needs to chase the fish, so the fish's column is the
            // hold/release target — not the bar center.
            let mut best_player_x = xs;
            let mut best_player_score: u64 = 0;
            let mut best_fish_x = xs;
            let mut best_fish_score: u64 = 0;
            for px in xs..xe {
                let mut col_white: u64 = 0;
                let mut col_color: u64 = 0;
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
                    let r = p[0] as u64;
                    let g = p[1] as u64;
                    let b = p[2] as u64;
                    let mn = r.min(g).min(b);
                    let mx = r.max(g).max(b);
                    // Bright grayscale: all three channels high AND similar.
                    // mn*3 ≈ brightness when grayscale, ≈ 0 when saturated.
                    col_white += mn * 3;
                    // Bright saturation: large gap between max and min channel,
                    // weighted by overall brightness so dim colored noise
                    // doesn't dominate.
                    let sat = mx - mn;
                    col_color += sat * (r + g + b) / 765;
                }
                if col_white > best_player_score {
                    best_player_score = col_white;
                    best_player_x = px;
                }
                if col_color > best_fish_score {
                    best_fish_score = col_color;
                    best_fish_x = px;
                }
            }
            if best_player_score > 0 {
                out.player_x = Some(best_player_x);
            }
            // Use the fish column as the target if we found one; fall back
            // to bar center if the fish detection didn't fire (e.g. between
            // minigame frames when the fish isn't on screen).
            if best_fish_score > 0 {
                out.fish_x = Some(best_fish_x);
                out.target_x = Some(best_fish_x);
            } else if best_player_score > 0 {
                out.target_x = Some(fb.x + (fb.width as i32) / 2);
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
            capture_region,
            click_at,
            mouse_move,
            mouse_down,
            mouse_up,
            find_player_x,
            detect_shake,
            capture_shake_baseline,
            clear_shake_baseline,
            has_shake_baseline,
            capture_shake_template,
            clear_shake_template,
            has_shake_template,
            save_shake_template_image,
            debug_save_cg_full,
            debug_save_full_with_marker,
            save_shake_snapshot,
            tick_macro,
            load_regions,
            save_regions,
            get_screen_size
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
