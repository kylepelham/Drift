//! The top-level windows of a browser Drift started: kept off-screen, and lifted above the rest for a moment so a
//! driver that searches windows in z-order finds this browser's first.

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowPlacement, GetWindowRect, GetWindowThreadProcessId, HWND_NOTOPMOST, HWND_TOPMOST,
        IsIconic, IsWindowVisible, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
        SetWindowPlacement, SetWindowPos, WINDOWPLACEMENT,
    };
    use windows_sys::core::BOOL;

    /// Where a window is out of sight on any monitor layout.
    const AWAY: i32 = -32000;
    /// A window left of this is already out of sight.
    const HIDDEN_LEFT: i32 = -20000;

    struct Search {
        pid: u32,
        found: Vec<HWND>,
    }

    unsafe extern "system" fn collect(window: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `Search` that `windows_of` passes, alive for the whole enumeration.
        let search = unsafe { &mut *(data as *mut Search) };
        let mut pid = 0;
        // SAFETY: `window` comes from the enumeration and `pid` is a valid output location.
        unsafe { GetWindowThreadProcessId(window, &mut pid) };
        // SAFETY: as above.
        if pid == search.pid && unsafe { IsWindowVisible(window) } != 0 {
            search.found.push(window);
        }

        1
    }

    fn windows_of(pid: u32) -> Vec<HWND> {
        let mut search = Search { pid, found: Vec::new() };
        // SAFETY: the callback only reads windows and writes into `search`, which outlives the call.
        unsafe { EnumWindows(Some(collect), &mut search as *mut Search as LPARAM) };

        search.found
    }

    pub(super) fn hide_windows(pid: u32) {
        for window in windows_of(pid) {
            // SAFETY: `window` was just enumerated.
            if unsafe { IsIconic(window) } != 0 {
                restore_away(window);
                continue;
            }
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            // SAFETY: `window` was just enumerated and `rect` is a valid output location.
            let read = unsafe { GetWindowRect(window, &mut rect) };
            if read == 0 || rect.left < HIDDEN_LEFT {
                continue;
            }
            // SAFETY: moving a window changes nothing but its position; a closed one makes the call fail.
            unsafe {
                SetWindowPos(
                    window,
                    std::ptr::null_mut(),
                    AWAY,
                    AWAY,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                )
            };
        }
    }

    /// Shows a minimized window at its normal size but out of sight, without it ever appearing on a screen.
    fn restore_away(window: HWND) {
        // SAFETY: WINDOWPLACEMENT is plain data, sized as the call requires.
        let mut placement: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
        placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        // SAFETY: `window` was just enumerated and `placement` carries its own size.
        if unsafe { GetWindowPlacement(window, &mut placement) } == 0 {
            return;
        }

        let normal = placement.rcNormalPosition;
        placement.rcNormalPosition = RECT {
            left: AWAY,
            top: AWAY,
            right: AWAY + (normal.right - normal.left),
            bottom: AWAY + (normal.bottom - normal.top),
        };
        placement.showCmd = SW_SHOWNOACTIVATE as u32;
        placement.flags = 0;
        // SAFETY: as above.
        unsafe { SetWindowPlacement(window, &placement) };
    }

    pub(super) fn set_topmost(pid: u32, on: bool) {
        let after = if on { HWND_TOPMOST } else { HWND_NOTOPMOST };
        for window in windows_of(pid) {
            // SAFETY: as in `hide_windows`; only the z-order changes.
            unsafe { SetWindowPos(window, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) };
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub(super) fn hide_windows(_pid: u32) {}

    pub(super) fn set_topmost(_pid: u32, _on: bool) {}
}

/// Moves every visible window of `pid` that is on a screen, or minimized, to where no one sees it.
pub fn hide_windows(pid: u32) {
    imp::hide_windows(pid);
}

/// Puts the visible windows of `pid` above every other window, or back among them.
pub fn set_topmost(pid: u32, on: bool) {
    imp::set_topmost(pid, on);
}
