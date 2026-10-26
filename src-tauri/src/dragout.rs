//! OLE drag source for dragging list rows out to Explorer and other shell targets.
//!
//! The list lives in WebView2, so the gesture starts in JS (pointer events) and hands off
//! here once the pointer leaves the window: `DoDragDrop` then owns the mouse until the
//! drop completes. The data object offers `CF_HDROP` (file paths) plus
//! `Preferred DropEffect`, which is what Explorer, the desktop and every `IDropTarget`
//! that accepts files look for.

use std::mem::size_of;
use std::sync::{LazyLock, Mutex};

use windows::core::{implement, Result as WinResult, BOOL, HRESULT};
use windows::Win32::Foundation::{
    GlobalFree, DATA_S_SAMEFORMATETC, DRAGDROP_S_CANCEL, DRAGDROP_S_USEDEFAULTCURSORS,
    DV_E_FORMATETC, E_OUTOFMEMORY, HGLOBAL, OLE_E_ADVISENOTSUPPORTED, POINT, S_FALSE, S_OK,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC,
    IEnumFORMATETC_Impl, DATADIR_GET, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0,
    TYMED_HGLOBAL, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE, GMEM_ZEROINIT,
};
use windows::Win32::System::Ole::{
    DoDragDrop, IDropSource, IDropSource_Impl, CF_HDROP, DROPEFFECT, DROPEFFECT_COPY,
    DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE,
};
use windows::Win32::UI::Shell::DROPFILES;

use crate::error::ApiError;

/// Which effects the user may choose at the drop target; Explorer narrows them by modifier.
const ALLOWED: u32 = DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0 | DROPEFFECT_LINK.0;

/// `CFSTR_PREFERREDDROPEFFECT`, registered once because the id is process-wide.
static DRAG_EFFECT_FORMAT: LazyLock<u16> =
    LazyLock::new(|| unsafe { RegisterClipboardFormatW(windows::core::w!("Preferred DropEffect")) as u16 });

fn format(cf: u16) -> FORMATETC {
    FORMATETC {
        cfFormat: cf,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

fn copy_to_hglobal(bytes: &[u8]) -> WinResult<HGLOBAL> {
    unsafe {
        // GMEM_ZEROINIT keeps the DROPFILES padding and terminators deterministic.
        let handle = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes.len())
            .map_err(|_| windows::core::Error::from_hresult(E_OUTOFMEMORY))?;
        let target = GlobalLock(handle);
        if target.is_null() {
            let _ = GlobalFree(Some(handle));
            return Err(windows::core::Error::from_hresult(E_OUTOFMEMORY));
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), target as *mut u8, bytes.len());
        let _ = GlobalUnlock(handle);
        Ok(handle)
    }
}

/// `DROPFILES` header followed by the double-null-terminated wide path list.
fn hdrop_bytes(paths: &[String]) -> Vec<u8> {
    let header = DROPFILES {
        pFiles: size_of::<DROPFILES>() as u32,
        pt: POINT { x: 0, y: 0 },
        fNC: BOOL(0),
        fWide: BOOL(1),
    };
    let mut bytes = vec![0u8; size_of::<DROPFILES>()];
    unsafe {
        std::ptr::copy_nonoverlapping(
            &header as *const DROPFILES as *const u8,
            bytes.as_mut_ptr(),
            size_of::<DROPFILES>(),
        );
    }
    for path in paths {
        for unit in path.encode_utf16() {
            bytes.extend_from_slice(&unit.to_ne_bytes());
        }
        bytes.extend_from_slice(&0u16.to_ne_bytes());
    }
    bytes.extend_from_slice(&0u16.to_ne_bytes());
    bytes
}

#[implement(IDataObject)]
struct DragData {
    paths: Vec<String>,
}

impl IDataObject_Impl for DragData_Impl {
    fn GetData(&self, format_in: *const FORMATETC) -> WinResult<STGMEDIUM> {
        let format_in = unsafe { format_in.as_ref() }
            .ok_or_else(|| windows::core::Error::from_hresult(DV_E_FORMATETC))?;
        if format_in.dwAspect != DVASPECT_CONTENT.0
            || format_in.tymed & TYMED_HGLOBAL.0 as u32 == 0
        {
            return Err(windows::core::Error::from_hresult(DV_E_FORMATETC));
        }
        let bytes = if format_in.cfFormat == CF_HDROP.0 {
            hdrop_bytes(&self.paths)
        } else if format_in.cfFormat == *DRAG_EFFECT_FORMAT {
            ALLOWED.to_ne_bytes().to_vec()
        } else {
            return Err(windows::core::Error::from_hresult(DV_E_FORMATETC));
        };
        Ok(STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 {
                hGlobal: copy_to_hglobal(&bytes)?,
            },
            pUnkForRelease: std::mem::ManuallyDrop::new(None),
        })
    }

    fn GetDataHere(&self, _format: *const FORMATETC, _medium: *mut STGMEDIUM) -> WinResult<()> {
        Err(windows::core::Error::from_hresult(DV_E_FORMATETC))
    }

    fn QueryGetData(&self, format_in: *const FORMATETC) -> HRESULT {
        let Some(format_in) = (unsafe { format_in.as_ref() }) else {
            return DV_E_FORMATETC;
        };
        let known = format_in.cfFormat == CF_HDROP.0 || format_in.cfFormat == *DRAG_EFFECT_FORMAT;
        if known
            && format_in.dwAspect == DVASPECT_CONTENT.0
            && format_in.tymed & TYMED_HGLOBAL.0 as u32 != 0
        {
            S_OK
        } else {
            DV_E_FORMATETC
        }
    }

    fn GetCanonicalFormatEtc(
        &self,
        _format_in: *const FORMATETC,
        format_out: *mut FORMATETC,
    ) -> HRESULT {
        if let Some(format_out) = unsafe { format_out.as_mut() } {
            *format_out = FORMATETC::default();
        }
        DATA_S_SAMEFORMATETC
    }

    /// Targets may push `Preferred DropEffect`; the accepted effect is reported by
    /// `DoDragDrop` itself, so nothing needs to be stored here.
    fn SetData(
        &self,
        _format_in: *const FORMATETC,
        _medium: *const STGMEDIUM,
        _release: BOOL,
    ) -> WinResult<()> {
        Ok(())
    }

    fn EnumFormatEtc(&self, direction: u32) -> WinResult<IEnumFORMATETC> {
        if direction != DATADIR_GET.0 as u32 {
            return Err(windows::core::Error::from_hresult(DV_E_FORMATETC));
        }
        let formats = vec![format(CF_HDROP.0), format(*DRAG_EFFECT_FORMAT)];
        Ok(Formats {
            formats,
            index: Mutex::new(0),
        }
        .into())
    }

    fn DAdvise(
        &self,
        _format: *const FORMATETC,
        _flags: u32,
        _sink: windows::core::Ref<'_, IAdviseSink>,
    ) -> WinResult<u32> {
        Err(windows::core::Error::from_hresult(OLE_E_ADVISENOTSUPPORTED))
    }

    fn DUnadvise(&self, _connection: u32) -> WinResult<()> {
        Err(windows::core::Error::from_hresult(OLE_E_ADVISENOTSUPPORTED))
    }

    fn EnumDAdvise(&self) -> WinResult<windows::Win32::System::Com::IEnumSTATDATA> {
        Err(windows::core::Error::from_hresult(OLE_E_ADVISENOTSUPPORTED))
    }
}

#[implement(IEnumFORMATETC)]
struct Formats {
    formats: Vec<FORMATETC>,
    index: Mutex<usize>,
}

impl IEnumFORMATETC_Impl for Formats_Impl {
    fn Next(&self, count: u32, out: *mut FORMATETC, fetched: *mut u32) -> HRESULT {
        let mut index = self.index.lock().unwrap();
        let mut written = 0;
        while written < count && *index < self.formats.len() {
            if !out.is_null() {
                unsafe { out.add(written as usize).write(self.formats[*index]) };
            }
            *index += 1;
            written += 1;
        }
        if !fetched.is_null() {
            unsafe { *fetched = written };
        }
        if written == count {
            S_OK
        } else {
            S_FALSE
        }
    }

    fn Skip(&self, count: u32) -> WinResult<()> {
        let mut index = self.index.lock().unwrap();
        *index = (*index + count as usize).min(self.formats.len());
        Ok(())
    }

    fn Reset(&self) -> WinResult<()> {
        *self.index.lock().unwrap() = 0;
        Ok(())
    }

    fn Clone(&self) -> WinResult<IEnumFORMATETC> {
        Ok(Formats {
            formats: self.formats.clone(),
            index: Mutex::new(*self.index.lock().unwrap()),
        }
        .into())
    }
}

#[implement(IDropSource)]
struct DropSource;

impl IDropSource_Impl for DropSource_Impl {
    /// Esc cancels; every other decision belongs to OLE (button release, target effect).
    fn QueryContinueDrag(
        &self,
        escape: BOOL,
        _keys: windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS,
    ) -> HRESULT {
        if escape.as_bool() {
            DRAGDROP_S_CANCEL
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Runs a modal OLE drag on its own STA thread and reports the accepted effect.
pub fn drag_out(paths: &[String]) -> Result<String, ApiError> {
    let paths = paths.to_vec();
    let effect = std::thread::Builder::new()
        .name("flowdir-drag".into())
        .spawn(move || -> Result<DROPEFFECT, ApiError> {
            unsafe {
                CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                    .ok()
                    .map_err(|e| ApiError::new("COM", e.to_string()))?;
                let data: IDataObject = DragData { paths }.into();
                let source: IDropSource = DropSource.into();
                let mut effect = DROPEFFECT_NONE;
                let result = DoDragDrop(&data, &source, DROPEFFECT(ALLOWED), &mut effect);
                CoUninitialize();
                result
                    .ok()
                    .map_err(|e| ApiError::from_windows("", e.code().0))
                    .map(|()| effect)
            }
        })
        .map_err(|e| ApiError::new("THREAD", e.to_string()))?
        .join()
        .map_err(|_| ApiError::new("THREAD", "drag thread panicked"))??;
    Ok(match effect {
        DROPEFFECT_COPY => "copy",
        DROPEFFECT_MOVE => "move",
        DROPEFFECT_LINK => "link",
        _ => "none",
    }
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Ole::ReleaseStgMedium;

    /// The payload is what every target reads: DROPFILES header, wide paths, double null.
    #[test]
    fn hdrop_payload_lists_every_path() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let paths = vec!["C:\\a\\b.txt".to_string(), "C:\\目录".to_string()];
        let data: IDataObject = DragData { paths: paths.clone() }.into();

        let hdrop = format(CF_HDROP.0);
        unsafe {
            assert_eq!(data.QueryGetData(&hdrop), S_OK);
            assert_eq!(data.QueryGetData(&format(0xFFFF)), DV_E_FORMATETC);
            assert_eq!(data.QueryGetData(&format(*DRAG_EFFECT_FORMAT)), S_OK);
        }

        let mut medium = unsafe { data.GetData(&hdrop) }.expect("CF_HDROP payload");
        assert_eq!(medium.tymed, TYMED_HGLOBAL.0 as u32);
        unsafe {
            let handle = medium.u.hGlobal;
            let start = GlobalLock(handle) as *const u8;
            let header = &*(start as *const DROPFILES);
            assert_eq!(header.pFiles as usize, size_of::<DROPFILES>());
            assert!(header.fWide.as_bool());
            let mut at = start.add(size_of::<DROPFILES>()) as *const u16;
            let mut seen = Vec::new();
            loop {
                let mut units = Vec::new();
                while *at != 0 {
                    units.push(*at);
                    at = at.add(1);
                }
                at = at.add(1);
                if units.is_empty() {
                    break;
                }
                seen.push(String::from_utf16(&units).unwrap());
            }
            assert_eq!(seen, paths);
            let _ = GlobalUnlock(handle);
            ReleaseStgMedium(&mut medium);
        }
        unsafe {
            CoUninitialize();
        }
    }

    /// The drop effect payload is the allowed-effect mask; Explorer reads it to pick the
    /// default copy/move semantics.
    #[test]
    fn preferred_drop_effect_is_offered() {
        let data: IDataObject = DragData { paths: vec!["C:\\x".into()] }.into();
        let mut medium = unsafe { data.GetData(&format(*DRAG_EFFECT_FORMAT)) }
            .expect("drop effect payload");
        unsafe {
            let handle = medium.u.hGlobal;
            let value = *(GlobalLock(handle) as *const u32);
            assert_eq!(value, ALLOWED);
            let _ = GlobalUnlock(handle);
            ReleaseStgMedium(&mut medium);
        }
    }
}
