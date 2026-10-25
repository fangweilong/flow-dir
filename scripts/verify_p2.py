"""End-to-end verification for P2: persistence, watcher, clipboard, settings."""
import asyncio
import base64
import ctypes
import ctypes.wintypes as wt
import json
import os
import subprocess
import sys
import time
import urllib.request

import websockets

TITLE = "FlowDir"
WATCH_DIR = os.path.join(os.environ["USERPROFILE"], "flowdir-watch")

user32 = ctypes.windll.user32


def find_window() -> int:
    found = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def enum_proc(hwnd, _):
        if not user32.IsWindowVisible(hwnd):
            return True
        buf = ctypes.create_unicode_buffer(512)
        user32.GetWindowTextW(hwnd, buf, 512)
        if buf.value == TITLE:
            found.append(hwnd)
            return False
        return True

    user32.EnumWindows(enum_proc, 0)
    if not found:
        raise SystemExit("window not found")
    return found[0]


def page_target() -> str:
    targets = json.load(urllib.request.urlopen("http://127.0.0.1:9333/json/list", timeout=10))
    return next(t for t in targets if t.get("type") == "page")["webSocketDebuggerUrl"]


SETUP_JS = r"""
(async () => {
  const raf = () => new Promise((r) => requestAnimationFrame(r));
  const wait = async (cond, ms = 15000) => {
    const t0 = performance.now();
    while (!cond()) {
      if (performance.now() - t0 > ms) throw new Error('timeout');
      await raf();
    }
  };
  const count = (sel) => document.querySelectorAll(sel).length;
  const setFilter = (pane, value) => {
    const input = pane.querySelector('input[placeholder="筛选"], input[placeholder="Filter"]');
    if (!input) throw new Error('no filter input');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(input, value);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  };

  document.querySelector('[title="左右双窗格"], [title="Two panes (side by side)"]').click();
  await wait(() => count('[data-pane-id]') === 2);
  await raf();

  const pane = document.querySelectorAll('[data-pane-id]')[0];
  pane.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
  await raf();
  setFilter(pane, 'flowdir-watch');
  await wait(() => pane.querySelector('[data-row="flowdir-watch"]'));
  pane.querySelector('[data-row="flowdir-watch"]').dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
  setFilter(pane, '');
  await wait(() => pane.querySelector('[data-status]').innerText.includes('项') || pane.querySelector('[data-status]').innerText.includes('items'));
  await raf();
  const rowsBefore = pane.querySelectorAll('[data-row]').length;

  // sort the second pane by size desc
  const pane2 = document.querySelectorAll('[data-pane-id]')[1];
  pane2.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
  await raf();
  const sizeHeader = Array.from(pane2.querySelectorAll('button')).find((b) => /大小|Size/.test(b.textContent));
  if (sizeHeader) { sizeHeader.dispatchEvent(new MouseEvent('click', { bubbles: true })); await raf(); }
  const sort = pane2.innerText.match(/(大小|Size)/) ? 'clicked' : 'missing';

  return { panes: count('[data-pane-id]'), rowsBefore, sort };
})()
"""

WATCH_JS = r"""
(async () => {
  const raf = () => new Promise((r) => requestAnimationFrame(r));
  const pane = document.querySelectorAll('[data-pane-id]')[0];
  const before = pane.querySelectorAll('[data-row]').length;
  const t0 = performance.now();
  while (performance.now() - t0 < 6000) {
    await raf();
    if (pane.querySelectorAll('[data-row]').length !== before) break;
  }
  return { before, after: pane.querySelectorAll('[data-row]').length, waited: Math.round(performance.now() - t0) };
})()
"""

CLIPBOARD_JS = r"""
(async () => {
  const raf = () => new Promise((r) => requestAnimationFrame(r));
  const pane = document.querySelectorAll('[data-pane-id]')[0];
  const rows = pane.querySelectorAll('[data-row]');
  const rect = (el) => { const r = el.getBoundingClientRect(); return { x: r.x + 60, y: r.y + r.height / 2 }; };
  return { count: rows.length, first: rect(rows[0]), second: rect(rows[1]), names: [rows[0].dataset.row, rows[1].dataset.row] };
})()
"""

SETTINGS_JS = r"""
(async () => {
  const raf = () => new Promise((r) => requestAnimationFrame(r));
  document.querySelector('header button[title="设置"], header button[title="Settings"]').click();
  await raf();
  await raf();
  const modal = document.querySelector('.fixed.inset-0');
  const dark = Array.from(modal.querySelectorAll('button')).find((b) => b.textContent.trim() === '深色' || b.textContent.trim() === 'Dark');
  dark.click();
  await raf();
  await new Promise((r) => setTimeout(r, 250));
  const isDark = document.documentElement.classList.contains('dark');
  const en = Array.from(modal.querySelectorAll('button')).find((b) => b.textContent.trim() === 'English');
  en.click();
  await raf();
  await new Promise((r) => setTimeout(r, 250));
  const headerText = document.querySelector('header').innerText;
  return { isDark, headerText: headerText.replace(/\n/g, ' | ') };
})()
"""

CLOSE_JS = r"""
(() => {
  const tabs = document.querySelectorAll('[data-tab]');
  return { tabs: tabs.length, status: document.querySelector('[data-status]').innerText };
})()
"""


async def main() -> None:
    os.makedirs(WATCH_DIR, exist_ok=True)
    for name in os.listdir(WATCH_DIR):
        os.remove(os.path.join(WATCH_DIR, name))
    for index in range(3):
        with open(os.path.join(WATCH_DIR, f"seed-{index}.txt"), "w", encoding="utf-8") as handle:
            handle.write(f"seed {index}\n")

    async with websockets.connect(page_target(), max_size=32 * 1024 * 1024) as ws:
        counter = 0

        async def cmd(method: str, params: dict | None = None) -> dict:
            nonlocal counter
            counter += 1
            await ws.send(json.dumps({"id": counter, "method": method, "params": params or {}}))
            while True:
                message = json.loads(await ws.recv())
                if message.get("id") == counter:
                    if "error" in message:
                        raise RuntimeError(message["error"])
                    return message.get("result", {})

        async def evaluate(expression: str):
            result = await cmd("Runtime.evaluate", {"expression": expression, "awaitPromise": True, "returnByValue": True})
            if "exceptionDetails" in result:
                raise RuntimeError(json.dumps(result["exceptionDetails"], ensure_ascii=False))
            return result["result"].get("value")

        await cmd("Page.enable")
        await asyncio.sleep(0.6)

        setup = await evaluate(SETUP_JS)
        print("setup:", setup)
        await asyncio.sleep(1.0)

        layout_path = os.path.join(os.environ["APPDATA"], "dev.flowdir.app", "layout.json")
        with open(layout_path, encoding="utf-8") as handle:
            saved = json.load(handle)
        print("layout file:", {
            "version": saved.get("version"),
            "panes": len(saved.get("workspace", {}).get("panes", {})),
            "window": saved.get("window"),
        })

        open(os.path.join(WATCH_DIR, "added-by-watcher.txt"), "w", encoding="utf-8").write("hi")
        watch = await evaluate(WATCH_JS)
        print("watcher:", watch)

        clip = await evaluate(CLIPBOARD_JS)
        print("clipboard rows:", clip["names"])
        for point, modifiers in ((clip["first"], 0), (clip["second"], 2)):
            for kind in ("mousePressed", "mouseReleased"):
                await cmd("Input.dispatchMouseEvent", {"type": kind, "x": point["x"], "y": point["y"], "button": "left", "clickCount": 1, "modifiers": modifiers})
        await cmd("Input.dispatchKeyEvent", {"type": "rawKeyDown", "key": "c", "code": "KeyC", "windowsVirtualKeyCode": 67, "nativeVirtualKeyCode": 67, "modifiers": 2})
        await cmd("Input.dispatchKeyEvent", {"type": "keyUp", "key": "c", "code": "KeyC", "windowsVirtualKeyCode": 67, "nativeVirtualKeyCode": 67, "modifiers": 2})
        await asyncio.sleep(0.6)
        clip_out = subprocess.run(
            ["powershell", "-NoProfile", "-Command", "Get-Clipboard -Format FileDropList | ForEach-Object { $_.FullName }"],
            capture_output=True, text=True,
        ).stdout.strip()
        print("clipboard:", clip_out.replace("\n", " | "))

        settings = await evaluate(SETTINGS_JS)
        print("settings:", settings)
        shot = await cmd("Page.captureScreenshot", {"format": "png"})
        with open("dark-settings.png", "wb") as handle:
            handle.write(base64.b64decode(shot["data"]))

        close_info = await evaluate(CLOSE_JS)
        print("before close:", close_info)

    # graceful close: CloseRequested -> flush-layout -> close
    hwnd = find_window()
    user32.PostMessageW(hwnd, 0x0010, 0, 0)
    time.sleep(2.5)
    running = subprocess.run(["powershell", "-NoProfile", "-Command", "(Get-Process flowdir -ErrorAction SilentlyContinue) -ne $null"], capture_output=True, text=True).stdout.strip()
    print("still running:", running)
    with open(layout_path, encoding="utf-8") as handle:
        saved = json.load(handle)
    paths = [tab["path"] for tab in saved["workspace"]["tabs"].values()]
    print("layout after close:", {"panes": len(saved["workspace"]["panes"]), "paths": paths})
    with open(os.path.join(os.environ["APPDATA"], "dev.flowdir.app", "settings.json"), encoding="utf-8") as handle:
        print("settings file:", handle.read().replace("\n", " "))


asyncio.run(main())
