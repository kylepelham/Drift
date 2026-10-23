//! The main window opens where it was left: the same screen, position, size and maximized state, as long as
//! that spot is still on a screen. Bounds are physical pixels in the desktop's coordinates.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{Manager, PhysicalPosition, PhysicalSize, WebviewWindow, WindowEvent};

use crate::store::Store;

const SETTING: &str = "window_placement";
/// Dragging fires a move per pixel; the spot is saved once the window settles.
const SAVE_DELAY: Duration = Duration::from_millis(500);
/// How much of the top edge, where the window is dragged from, must land on one screen.
const GRIP_WIDTH: i64 = 120;
const GRIP_HEIGHT: i64 = 16;
/// The window's minimum size in tauri.conf.json, before the screen's scale.
const MIN_SIZE: (f64, f64) = (900.0, 600.0);

/// Where the window was when not maximized, and whether it was maximized.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

/// A screen's work area (without the taskbar) and its scale.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Screen {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
}

impl Screen {
    fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }

    /// The width and height of the part of the window's top edge on this screen.
    fn grip_overlap(&self, placement: &Placement) -> (i64, i64) {
        let left = i64::from(placement.x).max(i64::from(self.x));
        let right = (i64::from(placement.x) + i64::from(placement.width)).min(self.right());
        let top = i64::from(placement.y).max(i64::from(self.y));
        let bottom = (i64::from(placement.y) + GRIP_HEIGHT).min(self.bottom());

        ((right - left).max(0), (bottom - top).max(0))
    }
}

/// The saved spot fitted to the screens there are now: none when no screen holds enough of its top edge to drag
/// it by. A window larger than its screen shrinks to it and moves onto it.
pub(crate) fn fit(saved: Placement, screens: &[Screen]) -> Option<Placement> {
    if saved.width == 0 || saved.height == 0 {
        return None;
    }

    let screen = screens
        .iter()
        .filter(|screen| {
            let (width, height) = screen.grip_overlap(&saved);
            width >= GRIP_WIDTH.min(i64::from(saved.width)) && height >= GRIP_HEIGHT / 2
        })
        .max_by_key(|screen| screen.grip_overlap(&saved).0)?;

    let least = |logical: f64, room: u32| ((logical * screen.scale).round() as u32).min(room);
    let width = saved.width.clamp(least(MIN_SIZE.0, screen.width), screen.width);
    let height = saved.height.clamp(least(MIN_SIZE.1, screen.height), screen.height);
    if (width, height) == (saved.width, saved.height) {
        return Some(saved);
    }

    let x = i64::from(saved.x).clamp(i64::from(screen.x), screen.right() - i64::from(width));
    let y = i64::from(saved.y).clamp(i64::from(screen.y), screen.bottom() - i64::from(height));
    Some(Placement {
        x: x as i32,
        y: y as i32,
        width,
        height,
        ..saved
    })
}

/// The window's last spot, kept as it moves and saved once it settles.
#[derive(Default)]
pub(crate) struct Tracker {
    current: Mutex<Option<Placement>>,
    moves: AtomicU64,
}

/// Puts the window back where it was, before it is first shown; false when there is no usable saved spot.
pub(crate) fn restore(window: &WebviewWindow) -> bool {
    let Some(saved) = window
        .state::<Store>()
        .app_setting(SETTING)
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str::<Placement>(&json).ok())
    else {
        return false;
    };
    let screens: Vec<Screen> = window
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|monitor| {
            let area = monitor.work_area();
            Screen {
                x: area.position.x,
                y: area.position.y,
                width: area.size.width,
                height: area.size.height,
                scale: monitor.scale_factor(),
            }
        })
        .collect();
    let Some(placement) = fit(saved, &screens) else {
        return false;
    };

    // Position first: landing on a screen with another scale resizes the window, and the size then corrects it.
    let placed = window
        .set_position(PhysicalPosition::new(placement.x, placement.y))
        .is_ok()
        && window
            .set_size(PhysicalSize::new(placement.width, placement.height))
            .is_ok();
    *window.state::<Tracker>().current.lock().unwrap() = Some(placement);

    placed
}

/// Maximizes the window if it was left maximized; this shows it.
pub(crate) fn maximize_if_saved(window: &WebviewWindow) {
    let maximized = window
        .state::<Tracker>()
        .current
        .lock()
        .unwrap()
        .is_some_and(|placement| placement.maximized);
    if maximized {
        let _ = window.maximize();
    }
}

/// Follows the window's moves once it is shown, and saves where it settles and where it was when it closes.
pub(crate) fn track(window: &WebviewWindow, shown: fn() -> bool) {
    let watched = window.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Moved(_) | WindowEvent::Resized(_) if shown() => save_later(&watched),
        WindowEvent::CloseRequested { .. } => {
            observe(&watched);
            save(&watched);
        }
        WindowEvent::Destroyed => save(&watched),
        _ => {}
    });
}

/// Records the window's spot; a maximized window keeps the bounds it will restore to, and a minimized one is skipped.
/// Read only once the window has settled: maximizing reports its full-screen move before the window counts as
/// maximized, which saved the whole screen as the size to restore to.
fn observe(window: &WebviewWindow) {
    if window.is_minimized().unwrap_or(true) {
        return;
    }
    let tracker = window.state::<Tracker>();
    let mut current = tracker.current.lock().unwrap();

    if window.is_maximized().unwrap_or(false) {
        if let Some(placement) = current.as_mut() {
            placement.maximized = true;
        }
        return;
    }
    let (Ok(position), Ok(size)) = (window.outer_position(), window.outer_size()) else {
        return;
    };
    *current = Some(Placement {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        maximized: false,
    });
}

fn save_later(window: &WebviewWindow) {
    let moves = window.state::<Tracker>().moves.fetch_add(1, Ordering::SeqCst) + 1;
    let window = window.clone();

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SAVE_DELAY).await;
        if window.state::<Tracker>().moves.load(Ordering::SeqCst) == moves {
            let _ = tauri::async_runtime::spawn_blocking(move || {
                observe(&window);
                save(&window);
            })
            .await;
        }
    });
}

fn save(window: &WebviewWindow) {
    let Some(placement) = *window.state::<Tracker>().current.lock().unwrap() else {
        return;
    };
    if let Ok(json) = serde_json::to_string(&placement) {
        let _ = window.state::<Store>().save_app_setting(SETTING, &json);
    }
}

#[cfg(test)]
#[path = "placement_tests.rs"]
mod tests;
