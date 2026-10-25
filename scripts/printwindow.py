"""Capture a window via PrintWindow (works without an interactive screen grab)."""
import ctypes
import ctypes.wintypes as wt
import sys

from PIL import Image

user32 = ctypes.windll.user32
gdi32 = ctypes.windll.gdi32
ctypes.windll.shcore.SetProcessDpiAwareness(2)

TITLE = "FlowDir"
matches = []


@ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
def enum_proc(hwnd, _):
    if not user32.IsWindowVisible(hwnd):
        return True
    buf = ctypes.create_unicode_buffer(512)
    user32.GetWindowTextW(hwnd, buf, 512)
    if buf.value == TITLE:
        matches.append(hwnd)
    return True


user32.EnumWindows(enum_proc, 0)
if not matches:
    sys.exit("FlowDir window not found")

hwnd = matches[0]
rect = wt.RECT()
user32.GetWindowRect(hwnd, ctypes.byref(rect))
w, h = rect.right - rect.left, rect.bottom - rect.top

hwnd_dc = user32.GetWindowDC(hwnd)
mem_dc = gdi32.CreateCompatibleDC(hwnd_dc)
bmp = gdi32.CreateCompatibleBitmap(hwnd_dc, w, h)
gdi32.SelectObject(mem_dc, bmp)
ok = user32.PrintWindow(hwnd, mem_dc, 2)

class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [
        ("biSize", wt.DWORD),
        ("biWidth", wt.LONG),
        ("biHeight", wt.LONG),
        ("biPlanes", wt.WORD),
        ("biBitCount", wt.WORD),
        ("biCompression", wt.DWORD),
        ("biSizeImage", wt.DWORD),
        ("biXPelsPerMeter", wt.LONG),
        ("biYPelsPerMeter", wt.LONG),
        ("biClrUsed", wt.DWORD),
        ("biClrImportant", wt.DWORD),
    ]

bi = BITMAPINFOHEADER()
bi.biSize = ctypes.sizeof(BITMAPINFOHEADER)
bi.biWidth = w
bi.biHeight = -h
bi.biPlanes = 1
bi.biBitCount = 32
bi.biCompression = 0

buf = ctypes.create_string_buffer(w * h * 4)
lines = gdi32.GetDIBits(mem_dc, bmp, 0, h, buf, ctypes.byref(bi), 0)
img = Image.frombuffer("RGB", (w, h), buf, "raw", "BGRX", 0, 1)
out = sys.argv[1] if len(sys.argv) > 1 else "shot.png"
img.save(out)
print(f"PrintWindow={ok} lines={lines} size={img.size}")
