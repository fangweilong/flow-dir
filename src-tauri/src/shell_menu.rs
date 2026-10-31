//! Native Explorer context menu (`IContextMenu`/`2`/`3`).
//!
//! The menu runs on a dedicated STA thread with a hidden owner window: shell
//! extensions (7-Zip, TortoiseGit, …) draw their submenus by handling
//! `WM_INITMENUPOPUP`/`WM_DRAWITEM`/`WM_MEASUREITEM`, so those messages must be
//! forwarded to `IContextMenu2::HandleMenuMsg` or the entries show up blank.

use std::cell::RefCell;
use std::ffi::{c_void, CString};
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use windows::core::{w, Interface, HSTRING, PCSTR, PCWSTR, PSTR, PWSTR};
use windows::Win32::Foundation::{HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    DeleteObject, MonitorFromPoint, HBITMAP, HGDIOBJ, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::Graphics::GdiPlus::{
    GdipCreateBitmapFromScan0, GdipCreateHBITMAPFromBitmap, GdipCreatePen1, GdipDeleteGraphics,
    GdipDeletePen, GdipDisposeImage, GdipDrawRectangleI, GdipGetImageGraphicsContext,
    GdipSetSmoothingMode, GdiplusStartup, GdiplusStartupInput, GpBitmap, GpGraphics, GpImage, GpPen,
    SmoothingModeAntiAlias, Status, UnitPixel,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, IBindCtx, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, GetSystemMetricsForDpi, SetThreadDpiAwarenessContext, MDT_EFFECTIVE_DPI,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Shell::{
    BHID_SFUIObject, IContextMenu, IContextMenu2, IContextMenu3, IShellFolder,
    SHBindToParent, SHCreateShellItemArrayFromIDLists, SHParseDisplayName, CMF_EXPLORE,
    CMF_EXTENDEDVERBS, CMF_NORMAL, CMIC_MASK_PTINVOKE, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX,
    GCS_VERBW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    GetMenuItemCount, GetMenuItemID, InsertMenuItemW, PostMessageW, RegisterClassW,
    SetForegroundWindow, TrackPopupMenuEx, HMENU, MENUITEMINFOW, MF_SEPARATOR, MF_STRING, MFT_STRING,
    MIIM_BITMAP, MIIM_ID, MIIM_STRING, MIIM_SUBMENU, SM_CXSMICON, SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TPM_WORKAREA, WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR, WM_NULL, WNDCLASSW,
    WS_POPUP,
};

use crate::error::ApiError;

const CMIC_MASK_UNICODE: u32 = 0x0000_4000;
const ID_CMD_FIRST: u32 = 1;
const ID_CMD_LAST: u32 = 0x7FFF;
/// Local ids live above the range handed to `QueryContextMenu`, so the shell never owns them.
const ID_CUSTOM_TOGGLE_HIDDEN: u32 = 0xC000;
const ID_CUSTOM_FIND_DUPLICATES: u32 = 0xC001;
const ID_CUSTOM_HASH: u32 = 0xC002;
const ID_CUSTOM_COMPARE: u32 = 0xC003;
const ID_CUSTOM_EXTRACT_HERE: u32 = 0xC010;
const ID_CUSTOM_EXTRACT_TO: u32 = 0xC011;
const ID_CUSTOM_COMPRESS_FIRST: u32 = 0xC020;
/// Formats the writer accepts; anything else in the label set is dropped.
const WRITABLE: [&str; 5] = ["zip", "7z", "tar.gz", "tar.xz", "tar.zst"];

/// GDI+'s `PixelFormat32bppARGB`; the crate exposes no constant for it.
const PIXEL_FORMAT_32BPP_ARGB: i32 = 0x0026_200A;
/// The app's accent (Tailwind blue-500 as ARGB), so FlowDir's entries share one mark.
const GLYPH_COLOR: u32 = 0xFF3B_82F6;

/// A FlowDir entry appended after the shell's own items. `action` is echoed to the
/// frontend, which owns the actual command; `glyph` marks the app's own features so
/// they do not read as one more shell verb.
struct Extra<'a> {
    id: u32,
    label: &'a str,
    action: &'static str,
    glyph: Option<MenuGlyph>,
}

#[derive(Clone, Copy)]
enum MenuGlyph {
    /// Two panels with a gap — the same mark the toolbar uses for compare.
    Compare,
    /// Two offset rectangles, read as "the same file twice".
    Duplicates,
}

/// Localized labels for the extraction submenu on an archive row.
pub struct ExtractLabels {
    pub title: String,
    pub here: String,
    pub to: String,
}

/// Localized labels for the compression submenu; `id` is the format the backend writes.
#[derive(Deserialize)]
pub struct CompressLabels {
    pub title: String,
    pub formats: Vec<CompressLabel>,
}

#[derive(Deserialize)]
pub struct CompressLabel {
    /// `zip` | `7z` | `tar.gz` | `tar.xz` | `tar.zst`.
    pub id: String,
    pub label: String,
}

/// Only a single archive file gets the submenu: folders, multi-selections and formats
/// without an in-process backend are left to the shell's own verbs.
fn extract_applies(paths: &[String]) -> bool {
    paths.len() == 1
        && crate::archive::format::from_name(&paths[0]).is_some()
        && std::fs::metadata(&paths[0])
            .map(|meta| !meta.is_dir())
            .unwrap_or(false)
}

/// Keeps the menu bitmaps alive while the popup is on screen: the menu does not copy them.
struct GlyphBitmaps(Vec<HBITMAP>);

impl Drop for GlyphBitmaps {
    fn drop(&mut self) {
        unsafe {
            for bitmap in &self.0 {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
            }
        }
    }
}

/// GDI+ is initialised once per process; the token deliberately lives to process exit.
fn gdiplus_ready() -> bool {
    static READY: OnceLock<bool> = OnceLock::new();
    *READY.get_or_init(|| unsafe {
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            ..Default::default()
        };
        let mut token = 0usize;
        GdiplusStartup(&mut token, &input, std::ptr::null_mut()) == Status(0)
    })
}

/// Renders `glyph` at `px` device pixels. Menu bitmaps are never DPI-scaled, so the
/// caller derives `px` from the monitor the popup opens on.
unsafe fn glyph_bitmap(glyph: MenuGlyph, px: i32) -> Option<HBITMAP> {
    unsafe {
        if !gdiplus_ready() {
            return None;
        }
        let mut bitmap: *mut GpBitmap = std::ptr::null_mut();
        if GdipCreateBitmapFromScan0(px, px, 0, PIXEL_FORMAT_32BPP_ARGB, None, &mut bitmap)
            != Status(0)
        {
            return None;
        }
        let mut graphics: *mut GpGraphics = std::ptr::null_mut();
        if GdipGetImageGraphicsContext(bitmap.cast::<GpImage>(), &mut graphics) != Status(0) {
            let _ = GdipDisposeImage(bitmap.cast::<GpImage>());
            return None;
        }
        let mut pen: *mut GpPen = std::ptr::null_mut();
        let scale = px as f32 / 16.0;
        if GdipCreatePen1(GLYPH_COLOR, 1.4 * scale, UnitPixel, &mut pen) == Status(0) {
            let _ = GdipSetSmoothingMode(graphics, SmoothingModeAntiAlias);
            // Coordinates come from the 16 px toolbar marks, scaled to the target size.
            let at = |v: i32| (v as f32 * scale).round() as i32;
            match glyph {
                MenuGlyph::Compare => {
                    let _ = GdipDrawRectangleI(graphics, pen, at(2), at(3), at(5), at(10));
                    let _ = GdipDrawRectangleI(graphics, pen, at(9), at(3), at(5), at(10));
                }
                MenuGlyph::Duplicates => {
                    let _ = GdipDrawRectangleI(graphics, pen, at(2), at(2), at(7), at(7));
                    let _ = GdipDrawRectangleI(graphics, pen, at(7), at(7), at(7), at(7));
                }
            }
            let _ = GdipDeletePen(pen);
        }
        let _ = GdipDeleteGraphics(graphics);
        let mut hbmp = HBITMAP::default();
        let status = GdipCreateHBITMAPFromBitmap(bitmap, &mut hbmp, 0);
        let _ = GdipDisposeImage(bitmap.cast::<GpImage>());
        (status == Status(0) && !hbmp.0.is_null()).then_some(hbmp)
    }
}

/// Small icon metrics for the monitor under the popup, in physical pixels.
unsafe fn glyph_size(x: i32, y: i32) -> i32 {
    unsafe {
        let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut dpi = 96u32;
        let mut ignored = 96u32;
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut ignored);
        GetSystemMetricsForDpi(SM_CXSMICON, dpi).max(16)
    }
}

/// `AppendMenuW` cannot attach a bitmap to a text item, so extras are inserted with a
/// `MENUITEMINFO`; the plain append stays as the fallback.
unsafe fn insert_extra(menu: HMENU, extra: &Extra<'_>, bitmap: Option<HBITMAP>) {
    unsafe {
        let wide = HSTRING::from(extra.label);
        let mut info = MENUITEMINFOW {
            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_ID | MIIM_STRING,
            fType: MFT_STRING,
            wID: extra.id,
            dwTypeData: PWSTR(wide.as_ptr() as *mut u16),
            cch: wide.len() as u32,
            ..Default::default()
        };
        if let Some(bitmap) = bitmap {
            info.fMask |= MIIM_BITMAP;
            info.hbmpItem = bitmap;
        }
        let index = GetMenuItemCount(Some(menu)).max(0) as u32;
        if InsertMenuItemW(menu, index, true, &info).is_err() {
            let _ = AppendMenuW(menu, MF_STRING, extra.id as usize, &wide);
        }
    }
}

/// Appends the compression leaves; returns (command id, format) so the caller can map the
/// reported command back to a format.
unsafe fn insert_compress_submenu(menu: HMENU, labels: &CompressLabels) -> Vec<(u32, String)> {
    unsafe {
        let Ok(submenu) = CreatePopupMenu() else {
            return Vec::new();
        };
        let mut mapped = Vec::new();
        for format in &labels.formats {
            if !WRITABLE.contains(&format.id.as_str()) || mapped.len() >= 8 {
                continue;
            }
            let id = ID_CUSTOM_COMPRESS_FIRST + mapped.len() as u32;
            let wide = HSTRING::from(format.label.as_str());
            if AppendMenuW(submenu, MF_STRING, id as usize, &wide).is_err() {
                continue;
            }
            mapped.push((id, format.id.clone()));
        }
        if mapped.is_empty() {
            let _ = DestroyMenu(submenu);
            return Vec::new();
        }
        let wide = HSTRING::from(labels.title.as_str());
        let info = MENUITEMINFOW {
            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_STRING | MIIM_SUBMENU,
            fType: MFT_STRING,
            dwTypeData: PWSTR(wide.as_ptr() as *mut u16),
            cch: wide.len() as u32,
            hSubMenu: submenu,
            ..Default::default()
        };
        let index = GetMenuItemCount(Some(menu)).max(0) as u32;
        if InsertMenuItemW(menu, index, true, &info).is_err() {
            let _ = DestroyMenu(submenu);
            return Vec::new();
        }
        mapped
    }
}

/// Appends FlowDir's extraction submenu; the two leaves return their own command ids,
/// which nested popups do report from `TrackPopupMenuEx`.
unsafe fn insert_submenu(menu: HMENU, labels: &ExtractLabels) {
    unsafe {
        let Ok(submenu) = CreatePopupMenu() else {
            return;
        };
        for (id, label) in [
            (ID_CUSTOM_EXTRACT_HERE, &labels.here),
            (ID_CUSTOM_EXTRACT_TO, &labels.to),
        ] {
            let wide = HSTRING::from(label.as_str());
            let _ = AppendMenuW(submenu, MF_STRING, id as usize, &wide);
        }
        let wide = HSTRING::from(labels.title.as_str());
        let info = MENUITEMINFOW {
            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_STRING | MIIM_SUBMENU,
            fType: MFT_STRING,
            dwTypeData: PWSTR(wide.as_ptr() as *mut u16),
            cch: wide.len() as u32,
            hSubMenu: submenu,
            ..Default::default()
        };
        let index = GetMenuItemCount(Some(menu)).max(0) as u32;
        if InsertMenuItemW(menu, index, true, &info).is_err() {
            let _ = DestroyMenu(submenu);
        }
    }
}

#[derive(Serialize, Clone)]
pub struct Outcome {
    pub invoked: bool,
    pub id: u32,
    /// Set when FlowDir's own appended entry was picked; shell commands leave it `None`.
    pub action: Option<String>,
    /// Archive format of a `compress` action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

thread_local! {
    static CONTEXT: RefCell<Option<(Option<IContextMenu2>, Option<IContextMenu3>)>> = const { RefCell::new(None) };
}

unsafe extern "system" fn owner_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if matches!(
        msg,
        WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR
    ) {
        let handled = CONTEXT.with(|cell| {
            let borrow = cell.borrow();
            let (menu2, menu3) = borrow.as_ref()?;
            unsafe {
                if let Some(menu3) = menu3 {
                    let mut result = LRESULT(0);
                    if menu3
                        .HandleMenuMsg2(msg, wparam, lparam, Some(&mut result))
                        .is_ok()
                    {
                        return Some(result);
                    }
                }
                if let Some(menu2) = menu2 {
                    let _ = menu2.HandleMenuMsg(msg, wparam, lparam);
                    return Some(LRESULT(0));
                }
            }
            None
        });
        if let Some(result) = handled {
            return result;
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Shows the shell menu at physical screen coordinates and waits until it closes.
/// `hash_label`, when present, appends FlowDir's checksum entry.
pub fn show(
    paths: Vec<String>,
    x: i32,
    y: i32,
    extended: bool,
    hash_label: Option<String>,
    extract: Option<ExtractLabels>,
    compress: Option<CompressLabels>,
) -> Result<Outcome, ApiError> {
    if paths.is_empty() {
        return Err(ApiError::new("EMPTY", "no paths"));
    }
    run_on_menu_thread(move || unsafe {
        run_items(
            &paths,
            x,
            y,
            extended,
            hash_label.as_deref(),
            extract.as_ref(),
            compress.as_ref(),
        )
    })
}

/// Shows Explorer's folder-background menu for `path`. `toggle_label`, `dup_label` and
/// `compare_label` are appended after the shell's entries because Explorer's own background
/// menu has none of them.
pub fn show_background(
    path: String,
    x: i32,
    y: i32,
    extended: bool,
    toggle_label: String,
    dup_label: String,
    compare_label: String,
) -> Result<Outcome, ApiError> {
    if path.is_empty() {
        return Err(ApiError::new("EMPTY", "no path"));
    }
    run_on_menu_thread(move || unsafe {
        run_background(&path, x, y, extended, &toggle_label, &dup_label, &compare_label)
    })
}

/// Runs the menu on its own STA thread: shell extensions expect one and the IPC thread must
/// stay free for the whole time the modal menu is open.
fn run_on_menu_thread(
    work: impl FnOnce() -> Result<Outcome, ApiError> + Send + 'static,
) -> Result<Outcome, ApiError> {
    run_offthread("flowdir-shell-menu", Duration::from_secs(900), work)?
}

/// Runs a blocking shell call off the IPC thread and waits for its result.
pub(crate) fn run_offthread<T: Send + 'static>(
    name: &str,
    timeout: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ApiError> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            let _ = tx.send(work());
        })
        .map_err(|e| ApiError::new("THREAD", e.to_string()))?;
    rx.recv_timeout(timeout)
        .map_err(|_| ApiError::new("TIMEOUT", "shell call did not return"))
}

unsafe fn run_items(
    paths: &[String],
    x: i32,
    y: i32,
    extended: bool,
    hash_label: Option<&str>,
    extract: Option<&ExtractLabels>,
    compress: Option<&CompressLabels>,
) -> Result<Outcome, ApiError> {
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| ApiError::new("COM", e.to_string()))?;
        let result = build_items(paths, x, y, extended, hash_label, extract, compress);
        CoUninitialize();
        result
    }
}

unsafe fn run_background(
    path: &str,
    x: i32,
    y: i32,
    extended: bool,
    toggle_label: &str,
    dup_label: &str,
    compare_label: &str,
) -> Result<Outcome, ApiError> {
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| ApiError::new("COM", e.to_string()))?;
        let result = build_background(path, x, y, extended, toggle_label, dup_label, compare_label);
        CoUninitialize();
        result
    }
}

unsafe fn build_items(
    paths: &[String],
    x: i32,
    y: i32,
    extended: bool,
    hash_label: Option<&str>,
    extract: Option<&ExtractLabels>,
    compress: Option<&CompressLabels>,
) -> Result<Outcome, ApiError> {
    unsafe {
        let mut pidls: Vec<*const windows::Win32::UI::Shell::Common::ITEMIDLIST> =
            Vec::with_capacity(paths.len());
        for path in paths {
            let wide = HSTRING::from(path);
            let mut pidl: *mut windows::Win32::UI::Shell::Common::ITEMIDLIST = std::ptr::null_mut();
            if let Err(err) = SHParseDisplayName(&wide, None::<&IBindCtx>, &mut pidl, 0, None) {
                // The guard was not built yet; free every PIDL parsed so far.
                drop(PidlGuard(pidls));
                return Err(ApiError::from_windows(path, err.code().0));
            }
            pidls.push(pidl as *const _);
        }
        let cleanup = PidlGuard(pidls.clone());

        let array = SHCreateShellItemArrayFromIDLists(&pidls)
            .map_err(|e| ApiError::from_windows("", e.code().0))?;
        let context: IContextMenu = array
            .BindToHandler(None::<&IBindCtx>, &BHID_SFUIObject)
            .map_err(|e| ApiError::new("SHELL_MENU", e.to_string()))?;

        let hwnd = create_owner().map_err(|e| ApiError::new("SHELL_MENU", e.to_string()))?;
        crate::theme::prepare_window(hwnd);
        let flags = CMF_NORMAL | if extended { CMF_EXTENDEDVERBS } else { 0 };
        let extras: Vec<Extra<'_>> = hash_label
            .map(|label| Extra {
                id: ID_CUSTOM_HASH,
                label,
                action: "hash",
                glyph: None,
            })
            .into_iter()
            .collect();
        let extract = extract.filter(|_| extract_applies(paths));
        let outcome = track(hwnd, &context, x, y, flags, &extras, extract, compress);
        drop(context);
        drop(array);
        drop(cleanup);
        outcome
    }
}

/// Resolves the folder's view object without showing anything; the caller owns `hwnd`, which
/// serves both as the menu owner and as the `CreateViewObject` owner.
unsafe fn background_context(
    path: &str,
    hwnd: HWND,
) -> Result<(IContextMenu, PidlGuard), ApiError> {
    unsafe {
        let wide = HSTRING::from(path);
        let mut pidl: *mut windows::Win32::UI::Shell::Common::ITEMIDLIST = std::ptr::null_mut();
        SHParseDisplayName(&wide, None::<&IBindCtx>, &mut pidl, 0, None)
            .map_err(|e| ApiError::from_windows(path, e.code().0))?;
        let cleanup = PidlGuard(vec![pidl as *const _]);

        // The folder's view object exposes Explorer's blank-area menu; the folder entry's
        // `BHID_SFUIObject` menu would only repeat the row menu.
        let mut child: *mut windows::Win32::UI::Shell::Common::ITEMIDLIST = std::ptr::null_mut();
        let parent: IShellFolder = SHBindToParent(pidl, Some(&mut child))
            .map_err(|e| ApiError::from_windows(path, e.code().0))?;
        if child.is_null() {
            return Err(ApiError::new("SHELL_MENU", "the desktop has no folder background"));
        }
        let folder: IShellFolder = parent
            .BindToObject(child, None::<&IBindCtx>)
            .map_err(|e| ApiError::from_windows(path, e.code().0))?;
        let context: IContextMenu = folder
            .CreateViewObject(hwnd)
            .map_err(|e| ApiError::new("SHELL_MENU", e.to_string()))?;
        Ok((context, cleanup))
    }
}

unsafe fn build_background(
    path: &str,
    x: i32,
    y: i32,
    extended: bool,
    toggle_label: &str,
    dup_label: &str,
    compare_label: &str,
) -> Result<Outcome, ApiError> {
    unsafe {
        let hwnd = create_owner().map_err(|e| ApiError::new("SHELL_MENU", e.to_string()))?;
        crate::theme::prepare_window(hwnd);
        let (context, cleanup) = match background_context(path, hwnd) {
            Ok(context) => context,
            Err(err) => {
                let _ = DestroyWindow(hwnd);
                return Err(err);
            }
        };
        let flags = CMF_NORMAL | CMF_EXPLORE | if extended { CMF_EXTENDEDVERBS } else { 0 };
        let extras = [
            Extra {
                id: ID_CUSTOM_FIND_DUPLICATES,
                label: dup_label,
                action: "findDuplicates",
                glyph: Some(MenuGlyph::Duplicates),
            },
            Extra {
                id: ID_CUSTOM_TOGGLE_HIDDEN,
                label: toggle_label,
                action: "toggleHidden",
                glyph: None,
            },
            Extra {
                id: ID_CUSTOM_COMPARE,
                label: compare_label,
                action: "compare",
                glyph: Some(MenuGlyph::Compare),
            },
        ];
        let outcome = track(hwnd, &context, x, y, flags, &extras, None, None);
        drop(context);
        drop(cleanup);
        outcome
    }
}

/// Builds, tracks and tears down the popup. Takes ownership of `hwnd`.
unsafe fn track(
    hwnd: HWND,
    context: &IContextMenu,
    x: i32,
    y: i32,
    flags: u32,
    extras: &[Extra<'_>],
    extract: Option<&ExtractLabels>,
    compress: Option<&CompressLabels>,
) -> Result<Outcome, ApiError> {
    unsafe {
        let menu2: Option<IContextMenu2> = context.cast().ok();
        let menu3: Option<IContextMenu3> = context.cast().ok();
        let menu = match CreatePopupMenu() {
            Ok(menu) => menu,
            Err(err) => {
                let _ = DestroyWindow(hwnd);
                return Err(ApiError::new("SHELL_MENU", err.to_string()));
            }
        };

        CONTEXT.with(|cell| *cell.borrow_mut() = Some((menu2, menu3)));

        let query = context.QueryContextMenu(menu, 0, ID_CMD_FIRST, ID_CMD_LAST, flags);
        if query.is_err() {
            CONTEXT.with(|cell| *cell.borrow_mut() = None);
            let _ = DestroyMenu(menu);
            let _ = DestroyWindow(hwnd);
            return Err(ApiError::new("SHELL_MENU", query.message()));
        }

        let mut bitmaps = GlyphBitmaps(Vec::new());
        if extract.is_some() || compress.is_some() || !extras.is_empty() {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        }
        if let Some(labels) = extract {
            insert_submenu(menu, labels);
        }
        let compress = compress.map(|labels| insert_compress_submenu(menu, labels)).unwrap_or_default();
        if !extras.is_empty() {
            let size = glyph_size(x, y);
            for extra in extras {
                let bitmap = extra.glyph.and_then(|glyph| glyph_bitmap(glyph, size));
                if let Some(bitmap) = bitmap {
                    bitmaps.0.push(bitmap);
                }
                insert_extra(menu, extra, bitmap);
            }
        }

        let _ = SetForegroundWindow(hwnd);
        let command = TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD.0 | TPM_RIGHTBUTTON.0 | TPM_WORKAREA.0,
            x,
            y,
            hwnd,
            None,
        )
        .0 as u32;
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));

        let mut outcome = Outcome {
            invoked: false,
            id: command,
            action: None,
            format: None,
        };
        if let Some((_, format)) = compress.iter().find(|(id, _)| *id == command) {
            outcome.action = Some("compress".into());
            outcome.format = Some(format.clone());
        } else if command == ID_CUSTOM_EXTRACT_HERE {
            outcome.action = Some("extractHere".into());
        } else if command == ID_CUSTOM_EXTRACT_TO {
            outcome.action = Some("extractTo".into());
        } else if let Some(extra) = extras.iter().find(|extra| extra.id == command) {
            outcome.action = Some(extra.action.into());
        } else if (ID_CMD_FIRST..=ID_CMD_LAST).contains(&command) {
            let offset = command - ID_CMD_FIRST;
            // New submenu entries carry canonical verbs such as `NewFolder`; remember the
            // verb so the frontend can select and rename the item the shell just created.
            let mut buffer = [0u16; 64];
            let verb = match context.GetCommandString(
                offset as usize,
                GCS_VERBW,
                None,
                PSTR(buffer.as_mut_ptr() as *mut u8),
                buffer.len() as u32,
            ) {
                Ok(()) => {
                    let len = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
                    String::from_utf16_lossy(&buffer[..len])
                }
                Err(_) => String::new(),
            };
            let info = CMINVOKECOMMANDINFOEX {
                cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
                fMask: CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE,
                hwnd,
                lpVerb: PCSTR(offset as usize as *const u8),
                lpVerbW: PCWSTR(offset as usize as *const u16),
                nShow: windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL.0,
                ptInvoke: windows::Win32::Foundation::POINT { x, y },
                ..Default::default()
            };
            let invoked = context.InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO);
            if let Err(err) = invoked {
                CONTEXT.with(|cell| *cell.borrow_mut() = None);
                let _ = DestroyMenu(menu);
                let _ = DestroyWindow(hwnd);
                return Err(ApiError::new("SHELL_MENU", err.to_string()));
            }
            outcome.invoked = true;
            if verb.to_ascii_lowercase().starts_with("new") {
                outcome.action = Some("newItem".into());
            }
        }

        CONTEXT.with(|cell| *cell.borrow_mut() = None);
        let _ = DestroyMenu(menu);
        let _ = DestroyWindow(hwnd);
        Ok(outcome)
    }
}

unsafe fn create_owner() -> windows::core::Result<HWND> {
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class_name = w!("FlowDirShellMenuOwner");
        let class = WNDCLASSW {
            lpfnWndProc: Some(owner_proc),
            hInstance: instance,
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassW(&class);
        CreateWindowExW(
            Default::default(),
            class_name,
            w!("FlowDir"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None::<HMENU>,
            Some(instance),
            None,
        )
    }
}

/// Opens the shell property sheet (`Alt+Enter`), independent of the menu path.
pub fn properties(hwnd: Option<isize>, path: &str) -> Result<(), ApiError> {
    use windows::Win32::UI::Shell::{SHObjectProperties, SHOP_FILEPATH};

    let wide = HSTRING::from(path);
    unsafe {
        let owner = hwnd.map(|raw| HWND(raw as *mut c_void));
        let ok = SHObjectProperties(owner, SHOP_FILEPATH, &wide, PCWSTR::null());
        if !ok.as_bool() {
            return Err(ApiError::new(
                "SHELL_FAILED",
                windows::core::Error::from_thread().to_string(),
            ));
        }
    }
    Ok(())
}

/// Invokes a shell verb by name on a drive root; used for `Eject` on removable media.
/// The canonical verb is locale-independent, unlike the menu text.
pub fn eject(path: String) -> Result<(), ApiError> {
    if path.is_empty() {
        return Err(ApiError::new("EMPTY", "no drive"));
    }
    run_offthread("flowdir-eject", Duration::from_secs(30), move || unsafe {
        run_eject(&path)
    })?
}

unsafe fn run_eject(path: &str) -> Result<(), ApiError> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| ApiError::new("COM", e.to_string()))?;
        let result = invoke_verb(path, "Eject");
        CoUninitialize();
        result
    }
}

unsafe fn invoke_verb(path: &str, verb: &str) -> Result<(), ApiError> {
    unsafe {
        let hwnd = create_owner().map_err(|e| ApiError::new("SHELL_MENU", e.to_string()))?;
        let result = invoke_verb_on(hwnd, path, verb);
        let _ = DestroyWindow(hwnd);
        result
    }
}

unsafe fn invoke_verb_on(hwnd: HWND, path: &str, verb: &str) -> Result<(), ApiError> {
    unsafe {
        let wide = HSTRING::from(path);
        let mut pidl: *mut windows::Win32::UI::Shell::Common::ITEMIDLIST = std::ptr::null_mut();
        SHParseDisplayName(&wide, None::<&IBindCtx>, &mut pidl, 0, None)
            .map_err(|e| ApiError::from_windows(path, e.code().0))?;
        let cleanup = PidlGuard(vec![pidl as *const _]);
        let mut child: *mut windows::Win32::UI::Shell::Common::ITEMIDLIST = std::ptr::null_mut();
        let parent: IShellFolder = SHBindToParent(pidl, Some(&mut child))
            .map_err(|e| ApiError::from_windows(path, e.code().0))?;
        if child.is_null() {
            return Err(ApiError::new("SHELL_MENU", "no shell item for the drive"));
        }
        let context: IContextMenu = parent
            .GetUIObjectOf(hwnd, &[child as *const _], None)
            .map_err(|e| ApiError::new("SHELL_MENU", e.to_string()))?;
        let menu = CreatePopupMenu().map_err(|e| ApiError::new("SHELL_MENU", e.to_string()))?;
        let outcome = invoke_from_menu(hwnd, &context, menu, &[verb]);
        let _ = DestroyMenu(menu);
        drop(context);
        drop(cleanup);
        outcome
    }
}

pub(crate) unsafe fn invoke_from_menu(
    hwnd: HWND,
    context: &IContextMenu,
    menu: HMENU,
    verbs: &[&str],
) -> Result<(), ApiError> {
    unsafe {
        // Verb discovery needs a built menu: handlers populate their verbs in QueryContextMenu.
        let query = context.QueryContextMenu(menu, 0, ID_CMD_FIRST, ID_CMD_LAST, CMF_NORMAL);
        if query.is_err() {
            return Err(ApiError::new("SHELL_MENU", "the drive exposes no context menu"));
        }
        let count = GetMenuItemCount(Some(menu));
        let mut found: Option<String> = None;
        for index in 0..count {
            let id = GetMenuItemID(menu, index);
            if !(ID_CMD_FIRST..=ID_CMD_LAST).contains(&id) {
                continue;
            }
            let mut buffer = [0u16; 64];
            if context
                .GetCommandString(
                    (id - ID_CMD_FIRST) as usize,
                    GCS_VERBW,
                    None,
                    PSTR(buffer.as_mut_ptr() as *mut u8),
                    buffer.len() as u32,
                )
                .is_err()
            {
                continue;
            }
            let len = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
            let found_verb = String::from_utf16_lossy(&buffer[..len]);
            if verbs.iter().any(|verb| found_verb.eq_ignore_ascii_case(verb)) {
                found = Some(found_verb);
                break;
            }
        }
        let Some(verb) = found else {
            return Err(ApiError::new("NO_VERB", format!("none of {verbs:?} for this item")));
        };
        // Both spellings name the verb: the shell accepts a string instead of an offset.
        let ansi = CString::new(verb.as_str()).map_err(|_| ApiError::new("BAD_VERB", &verb))?;
        let wide = HSTRING::from(verb.as_str());
        let info = CMINVOKECOMMANDINFOEX {
            cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
            fMask: CMIC_MASK_UNICODE,
            hwnd,
            lpVerb: PCSTR(ansi.as_ptr() as *const u8),
            lpParameters: PCSTR::null(),
            lpDirectory: PCSTR::null(),
            nShow: SW_SHOWNORMAL.0,
            dwHotKey: 0,
            hIcon: HANDLE::default(),
            lpTitle: PCSTR::null(),
            lpVerbW: PCWSTR(wide.as_ptr()),
            lpParametersW: PCWSTR::null(),
            lpDirectoryW: PCWSTR::null(),
            lpTitleW: PCWSTR::null(),
            ptInvoke: POINT { x: 0, y: 0 },
        };
        context
            .InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO)
            .map_err(|e| ApiError::new("SHELL_MENU", e.to_string()))
    }
}

struct PidlGuard(Vec<*const windows::Win32::UI::Shell::Common::ITEMIDLIST>);

impl Drop for PidlGuard {
    fn drop(&mut self) {
        unsafe {
            for pidl in &self.0 {
                CoTaskMemFree(Some(*pidl as *const c_void));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{GetMenuItemCount, GetMenuStringW, MF_BYPOSITION};

    /// The submenu is inserted with `MIIM_SUBMENU`, which `AppendMenuW` cannot express;
    /// a broken insert would silently leave the archive row without extraction.
    #[test]
    fn extraction_submenu_lands_after_the_shell_entries() {
        unsafe {
            let menu = CreatePopupMenu().expect("popup menu");
            insert_extra(
                menu,
                &Extra {
                    id: ID_CUSTOM_HASH,
                    label: "校验和",
                    action: "hash",
                    glyph: None,
                },
                None,
            );
            let labels = ExtractLabels {
                title: "解压".into(),
                here: "解压到当前文件夹".into(),
                to: "解压到…".into(),
            };
            insert_submenu(menu, &labels);
            assert_eq!(GetMenuItemCount(Some(menu)), 2);
            let mut buffer = [0u16; 64];
            let length = GetMenuStringW(menu, 1, Some(&mut buffer), MF_BYPOSITION);
            assert!(length > 0, "submenu title missing");
            assert_eq!(String::from_utf16_lossy(&buffer[..length as usize]), "解压");
            let _ = DestroyMenu(menu);
        }
    }

    /// Guards the integration point: the folder's view object must yield a populated
    /// background menu, otherwise the app silently falls back to the self-drawn menu.
    #[test]
    fn folder_background_exposes_shell_entries() {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                .ok()
                .expect("COM apartment");
            let hwnd = create_owner().expect("owner window");
            let dir = std::env::temp_dir();
            let mut count = -1;
            if let Ok((context, pidls)) = background_context(&dir.to_string_lossy(), hwnd) {
                let menu = CreatePopupMenu().expect("popup menu");
                let flags = CMF_NORMAL | CMF_EXPLORE;
                let query = context.QueryContextMenu(menu, 0, ID_CMD_FIRST, ID_CMD_LAST, flags);
                count = if query.is_ok() {
                    GetMenuItemCount(Some(menu))
                } else {
                    -2
                };
                let _ = DestroyMenu(menu);
                drop(context);
                drop(pidls);
            }
            let _ = DestroyWindow(hwnd);
            CoUninitialize();
            assert!(count > 0, "background menu must contain shell entries, got {count}");
        }
    }
}
