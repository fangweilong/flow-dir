//! App-theme propagation to Win32 surfaces (title bar, native menus, dialogs).
//!
//! Win32 has no supported per-window dark mode API. The de-facto approach is
//! uxtheme's undocumented ordinals (used by Explorer, Notepad, VS Code):
//! 135 `SetPreferredAppMode`, 136 `FlushMenuThemes`, 133
//! `AllowDarkModeForWindow`. They are resolved at runtime, so a future build
//! that drops them degrades to the OS default instead of failing to start.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::LazyLock;

use windows::core::{w, BOOL, PCSTR, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::UI::Controls::SetWindowTheme;

const MODE_DEFAULT: u8 = 0;
const MODE_DARK: u8 = 1;
const MODE_LIGHT: u8 = 2;

static CURRENT: AtomicU8 = AtomicU8::new(MODE_DEFAULT);

struct Apis {
    set_preferred_app_mode: Option<unsafe extern "system" fn(i32) -> i32>,
    flush_menu_themes: Option<unsafe extern "system" fn()>,
    allow_dark_mode_for_window: Option<unsafe extern "system" fn(HWND, BOOL) -> BOOL>,
}

fn apis() -> &'static Apis {
    static APIS: LazyLock<Apis> = LazyLock::new(|| unsafe {
        let module = GetModuleHandleW(w!("uxtheme.dll"))
            .or_else(|_| LoadLibraryW(w!("uxtheme.dll")))
            .ok();
        let Some(module) = module else {
            return Apis {
                set_preferred_app_mode: None,
                flush_menu_themes: None,
                allow_dark_mode_for_window: None,
            };
        };
        let by_ordinal =
            |ordinal: u16| GetProcAddress(module, PCSTR(ordinal as usize as *const u8));
        Apis {
            // SAFETY: the signatures match the uxtheme exports used by Explorer.
            set_preferred_app_mode: by_ordinal(135).map(|f| std::mem::transmute(f)),
            flush_menu_themes: by_ordinal(136).map(|f| std::mem::transmute(f)),
            allow_dark_mode_for_window: by_ordinal(133).map(|f| std::mem::transmute(f)),
        }
    });
    &APIS
}

/// Applies `theme` (`system` / `light` / `dark`) to the process and `hwnd`.
pub fn apply(hwnd: Option<HWND>, theme: &str) {
    let mode = match theme {
        "dark" => MODE_DARK,
        "light" => MODE_LIGHT,
        _ => MODE_DEFAULT,
    };
    CURRENT.store(mode, Ordering::Relaxed);
    let apis = apis();
    unsafe {
        if let Some(set_mode) = apis.set_preferred_app_mode {
            // 0 ForceDark? No: uxtheme's enum is Default=0, AllowDark=1, ForceDark=2,
            // ForceLight=3, Max=4. "system" follows the OS via AllowDark.
            let raw = match mode {
                MODE_DARK => 2,
                MODE_LIGHT => 3,
                _ => 1,
            };
            let _ = set_mode(raw);
        }
        if let Some(flush) = apis.flush_menu_themes {
            flush();
        }
    }
    if let Some(hwnd) = hwnd {
        prepare_window(hwnd);
    }
}

/// Must run for windows that own menus or dialogs so they pick up the mode.
pub fn prepare_window(hwnd: HWND) {
    let apis = apis();
    let mode = CURRENT.load(Ordering::Relaxed);
    if mode == MODE_DEFAULT {
        return;
    }
    unsafe {
        if let Some(allow) = apis.allow_dark_mode_for_window {
            let _ = allow(hwnd, BOOL(1));
        }
        let theme = if mode == MODE_DARK {
            w!("DarkMode_Explorer")
        } else {
            w!("Explorer")
        };
        let _ = SetWindowTheme(hwnd, theme, PCWSTR::null());
        let dark = BOOL(if mode == MODE_DARK { 1 } else { 0 });
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const BOOL as *const _,
            std::mem::size_of::<BOOL>() as u32,
        );
    }
}
