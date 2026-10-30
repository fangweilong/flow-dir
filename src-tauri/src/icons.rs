//! Shell icons for the file list (`fd://icon?...`).
//!
//! `IShellItemImageFactory::GetImage` with `SIIGBF_ICONONLY` returns exactly the
//! icon Explorer would show (per-extension, per-`.exe`, folders, shortcuts)
//! without loading thumbnail handlers.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::Path as FsPath;

use tauri::http::{header, Response, StatusCode};
use tauri::{AppHandle, Manager, Runtime};
use windows::core::{Interface, GUID, HSTRING};
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatPng, GUID_WICPixelFormat32bppBGRA,
    IWICBitmapEncoder, IWICBitmapFrameEncode, IWICImagingFactory, IWICStream,
    WICBitmapEncoderNoCache,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IStream, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    STATFLAG_NONAME, STREAM_SEEK_SET,
};
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF, SIIGBF_BIGGERSIZEOK,
    SIIGBF_ICONONLY,
};

use crate::preview::PreviewCache;

const PNG_BUFFER: usize = 256 * 1024;

/// HTTP response for `http://fd.localhost/icon?path=…&s=16`.
pub fn handle_request<R: Runtime>(app: &AppHandle<R>, uri: &tauri::http::Uri) -> Response<Vec<u8>> {
    let params: HashMap<String, String> = uri
        .query()
        .map(|query| {
            query
                .split('&')
                .filter_map(|pair| {
                    let (key, value) = pair.split_once('=')?;
                    let decode = |raw: &str| {
                        percent_encoding::percent_decode_str(raw)
                            .decode_utf8_lossy()
                            .into_owned()
                    };
                    Some((decode(key), decode(value)))
                })
                .collect()
        })
        .unwrap_or_default();

    let Some(path) = params.get("path") else {
        return plain(StatusCode::BAD_REQUEST, "missing path");
    };
    if path.is_empty() || path.len() > 1024 || !FsPath::new(path).is_absolute() {
        return plain(StatusCode::BAD_REQUEST, "invalid path");
    }
    if !FsPath::new(path).exists() {
        return plain(StatusCode::NOT_FOUND, "not found");
    }
    let size = params
        .get("s")
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(16)
        .clamp(16, 64);

    let key = format!(
        "icon:{}?s={size}&v={}",
        path,
        params.get("v").map(String::as_str).unwrap_or_default()
    );
    let cache = app.state::<PreviewCache>();
    if let Some(bytes) = cache.get(&key) {
        return ok(bytes);
    }
    match render(path, size) {
        Ok(bytes) => {
            cache.put(key, bytes.clone());
            ok(bytes)
        }
        Err(_) => plain(StatusCode::INTERNAL_SERVER_ERROR, "icon failed"),
    }
}

struct ComMtaGuard(bool);

impl ComMtaGuard {
    fn new() -> Self {
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            Self(hr.is_ok())
        }
    }
}

impl Drop for ComMtaGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

fn render(path: &str, size: i32) -> Result<Vec<u8>, windows::core::Error> {
    let _com = ComMtaGuard::new();
    unsafe {
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(&HSTRING::from(path), None)?;
        let flags = SIIGBF(SIIGBF_ICONONLY.0 | SIIGBF_BIGGERSIZEOK.0);
        let bitmap = factory.GetImage(SIZE { cx: size, cy: size }, flags)?;
        let result = bitmap_to_png(bitmap);
        let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(bitmap.0));
        result
    }
}

fn bitmap_to_png(bitmap: HBITMAP) -> Result<Vec<u8>, windows::core::Error> {
    unsafe {
        let mut info = BITMAP::default();
        if GetObjectW(
            windows::Win32::Graphics::Gdi::HGDIOBJ(bitmap.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut info as *mut _ as *mut c_void),
        ) == 0
        {
            return Err(windows::core::Error::from_thread());
        }
        let width = info.bmWidth.max(1);
        let height = info.bmHeight.max(1);
        let stride = (width * 4) as u32;
        let mut pixels = vec![0u8; (stride * height as u32) as usize];

        let mut dib = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let dc = GetDC(None);
        let lines = GetDIBits(
            dc,
            bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr() as *mut c_void),
            &mut dib,
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, dc);
        if lines == 0 {
            return Err(windows::core::Error::from_thread());
        }
        encode_png(width as u32, height as u32, &pixels)
    }
}

fn encode_png(width: u32, height: u32, bgra: &[u8]) -> Result<Vec<u8>, windows::core::Error> {
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let mut buffer = vec![0u8; PNG_BUFFER];
        let stream: IWICStream = factory.CreateStream()?;
        stream.InitializeFromMemory(&buffer)?;

        let encoder: IWICBitmapEncoder =
            factory.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;

        let mut frame: Option<IWICBitmapFrameEncode> = None;
        let mut options = None;
        encoder.CreateNewFrame(&mut frame, &mut options)?;
        let frame = frame.ok_or_else(windows::core::Error::from_thread)?;
        frame.Initialize(None)?;
        frame.SetSize(width, height)?;
        let mut format: GUID = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut format)?;
        frame.WritePixels(height, width * 4, bgra)?;
        frame.Commit()?;
        encoder.Commit()?;

        let istream: IStream = stream.cast()?;
        let mut stat = Default::default();
        istream.Stat(&mut stat, STATFLAG_NONAME)?;
        let size = stat.cbSize as usize;
        istream.Seek(0, STREAM_SEEK_SET, None)?;
        buffer.truncate(size);
        Ok(buffer)
    }
}

fn ok(body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/png")
        .header(header::CACHE_CONTROL, "public, max-age=86400")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(body)
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "encode"))
}

fn plain(status: StatusCode, message: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(message.as_bytes().to_vec())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}
