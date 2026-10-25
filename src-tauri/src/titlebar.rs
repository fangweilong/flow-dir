//! Polish for the undecorated window (`decorations: false`).
//!
//! Without a caption Windows drops the drop shadow and Win11 rounded corners,
//! so the window floats like a borderless popup. Two DWM knobs restore the
//! native look: `DWMWA_WINDOW_CORNER_PREFERENCE` for the radius and a 1-pixel
//! `DwmExtendFrameIntoClientArea` margin, which is the smallest nonzero inset
//! that makes DWM compose the frame (and therefore the shadow) again.

use tauri::WebviewWindow;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::WindowsAndMessaging::{
    EnableMenuItem, GetSystemMenu, PostMessageW, SetForegroundWindow, TrackPopupMenu, MF_BYCOMMAND,
    MF_ENABLED, MF_GRAYED, SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE, SC_MOVE, SC_RESTORE, SC_SIZE,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_NULL, WM_SYSCOMMAND,
};

const DWMWCP_ROUND: i32 = 2;

/// Applies rounded corners and shadow; call once the window exists.
pub fn apply(window: &WebviewWindow) {
    let Ok(raw) = window.hwnd() else { return };
    let hwnd = HWND(raw.0);
    unsafe {
        let pref = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const i32 as *const _,
            std::mem::size_of::<i32>() as u32,
        );
        set_shadow(hwnd, true);
    }
}

/// Pops the window's system menu (Alt+Space / right-click on the title bar).
///
/// The caption is drawn by the webview, so Windows never opens the system menu
/// on its own; without this the window would have no way to reach Move/Size/
/// Minimize. Menu state is synchronized first, otherwise a maximized window
/// still offers "Maximize".
pub fn show_system_menu(hwnd: HWND, maximized: bool, x: i32, y: i32) {
    unsafe {
        let menu = GetSystemMenu(hwnd, false);
        if menu.is_invalid() {
            return;
        }
        let flags = |on: bool| MF_BYCOMMAND | if on { MF_ENABLED } else { MF_GRAYED };
        let _ = EnableMenuItem(menu, SC_RESTORE, flags(maximized));
        let _ = EnableMenuItem(menu, SC_MAXIMIZE, flags(!maximized));
        let _ = EnableMenuItem(menu, SC_MINIMIZE, flags(true));
        let _ = EnableMenuItem(menu, SC_MOVE, flags(true));
        let _ = EnableMenuItem(menu, SC_SIZE, flags(!maximized));
        let _ = EnableMenuItem(menu, SC_CLOSE, flags(true));

        let _ = SetForegroundWindow(hwnd);
        // `TPM_RETURNCMD` makes `TrackPopupMenu` return the chosen id instead of a BOOL.
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            x,
            y,
            None,
            hwnd,
            None,
        )
        .0;
        let _ = PostMessageW(Some(hwnd), WM_NULL, Default::default(), Default::default());
        if command != 0 {
            let _ = PostMessageW(
                Some(hwnd),
                WM_SYSCOMMAND,
                windows::Win32::Foundation::WPARAM(command as usize),
                windows::Win32::Foundation::LPARAM(0),
            );
        }
    }
}

/// Maximized windows must not keep the inset frame, otherwise the 1px margin
/// lands on screen as a hairline gap around the working area.
pub fn set_shadow(hwnd: HWND, enabled: bool) {
    let margin = i32::from(enabled);
    let margins = MARGINS {
        cxLeftWidth: margin,
        cxRightWidth: margin,
        cyTopHeight: margin,
        cyBottomHeight: margin,
    };
    unsafe {
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
    }
}
