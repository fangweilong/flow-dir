"""Verify P3 preview pipeline and search over CDP."""
import asyncio
import base64
import json
import os
import sys
import urllib.request

import websockets

PREVIEW_DIR = os.path.join(os.environ["USERPROFILE"], "flowdir-preview")


def page_target() -> str:
    targets = json.load(urllib.request.urlopen("http://127.0.0.1:9333/json/list", timeout=10))
    return next(t for t in targets if t.get("type") == "page")["webSocketDebuggerUrl"]


HELPERS = r"""
const raf = () => new Promise((r) => requestAnimationFrame(r));
const wait = async (cond, ms = 15000) => {
  const t0 = performance.now();
  while (!cond()) {
    if (performance.now() - t0 > ms) throw new Error('timeout');
    await raf();
  }
};
const pane = () => document.querySelector('[data-pane-id]');
const setFilter = (value) => {
  const input = pane().querySelector('input[placeholder="筛选"], input[placeholder="Filter"]');
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
  setter.call(input, value);
  input.dispatchEvent(new Event('input', { bubbles: true }));
};
const openRow = async (name) => {
  setFilter(name);
  await wait(() => pane().querySelector('[data-row="' + name + '"]'));
  pane().querySelector('[data-row="' + name + '"]').dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
  setFilter('');
  await wait(() => pane().querySelector('[data-status]').innerText.includes('项'));
  await raf();
};
const clickRow = async (name) => {
  setFilter(name);
  await wait(() => pane().querySelector('[data-row="' + name + '"]'));
  const row = pane().querySelector('[data-row="' + name + '"]');
  const r = row.getBoundingClientRect();
  row.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, button: 0, clientX: r.x + 30, clientY: r.y + 10 }));
  row.dispatchEvent(new MouseEvent('mouseup', { bubbles: true, button: 0 }));
  setFilter('');
  await raf();
};
"""

NAV_JS = r"""
(async () => {
  HELPERS
  const home = Array.from(document.querySelectorAll('aside button')).find((b) => /主目录|Home/.test(b.innerText));
  if (home) {
    home.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await raf();
    await raf();
    await raf();
  }
  await openRow('flowdir-preview');
  return { path: pane().querySelector('[data-status]').innerText.split('\n').pop() };
})()
"""

PREVIEW_JS = r"""
(async () => {
  HELPERS
  const name = NAME;
  await clickRow(name);
  await wait(() => document.querySelector('aside.w-\\[320px\\]')?.innerText.includes(name));
  await wait(() => !document.querySelector('aside.w-\\[320px\\]')?.innerText.includes('正在生成预览'));
  const panel = document.querySelector('aside.w-\\[320px\\]');
  const img = panel.querySelector('img');
  if (img) {
    await new Promise((resolve) => { if (img.complete) resolve(); else { img.onload = resolve; img.onerror = resolve; } });
  }
  return {
    name,
    text: panel.innerText.replace(/\n/g, ' | ').slice(0, 220),
    imgSrc: img ? img.getAttribute('src') : null,
    imgWidth: img ? img.naturalWidth : 0,
    imgHeight: img ? img.naturalHeight : 0,
    pre: panel.querySelector('pre')?.innerText.slice(0, 120) ?? null,
    rows: panel.querySelectorAll('.space-y-0\\.5 > div, .space-y-1 > div').length,
  };
})()
"""

NAV_WINDOWS_JS = r"""
(async () => {
  HELPERS
  const crumb = Array.from(pane().querySelectorAll('button')).find((b) => b.textContent.trim() === 'C:');
  crumb.dispatchEvent(new MouseEvent('click', { bubbles: true }));
  await wait(() => pane().querySelector('[data-row="Windows"]'));
  await openRow('Windows');
  return pane().querySelector('[data-status]').innerText.split('\n').pop();
})()
"""

SEARCH_JS = r"""
(async () => {
  HELPERS
  return { opened: !!document.querySelector('input[placeholder*="搜索"], input[placeholder*="Search"]') };
})()
"""


async def main() -> None:
    async with websockets.connect(page_target(), max_size=32 * 1024 * 1024) as ws:
        counter = 0

        async def cmd(method, params=None):
            nonlocal counter
            counter += 1
            await ws.send(json.dumps({"id": counter, "method": method, "params": params or {}}))
            while True:
                message = json.loads(await ws.recv())
                if message.get("id") == counter:
                    if "error" in message:
                        raise RuntimeError(message["error"])
                    return message.get("result", {})

        async def evaluate(expression):
            wrapped = expression.replace("HELPERS", HELPERS)
            result = await cmd(
                "Runtime.evaluate",
                {"expression": wrapped, "awaitPromise": True, "returnByValue": True},
            )
            if "exceptionDetails" in result:
                raise RuntimeError(json.dumps(result["exceptionDetails"], ensure_ascii=False)[:800])
            return result["result"].get("value")

        await cmd("Page.enable")
        await asyncio.sleep(0.5)

        print("navigate:", await evaluate(NAV_JS))

        for name in ["demo.png", "demo.pdf", "demo.txt", "demo.zip"]:
            js = PREVIEW_JS.replace("NAME", json.dumps(name))
            print("preview:", await evaluate(js))

        shot = await cmd("Page.captureScreenshot", {"format": "png"})
        with open("preview-image.png", "wb") as handle:
            handle.write(base64.b64decode(shot["data"]))

        print("windows:", await evaluate(NAV_WINDOWS_JS))

        await cmd("Input.dispatchKeyEvent", {
            "type": "rawKeyDown", "key": "f", "code": "KeyF",
            "windowsVirtualKeyCode": 70, "nativeVirtualKeyCode": 70, "modifiers": 2,
        })
        await cmd("Input.dispatchKeyEvent", {
            "type": "keyUp", "key": "f", "code": "KeyF",
            "windowsVirtualKeyCode": 70, "nativeVirtualKeyCode": 70, "modifiers": 2,
        })
        await asyncio.sleep(0.4)
        await cmd("Input.insertText", {"text": "notepad"})
        await asyncio.sleep(2.5)

        results = await evaluate(r"""
        (() => {
          const rows = Array.from(document.querySelectorAll('[data-result]'));
          return {
            count: rows.length,
            first: rows[0]?.innerText.replace(/\n/g, ' | ') ?? null,
            badge: document.querySelector('.fixed.inset-0 .rounded.bg-surface-3')?.innerText ?? null,
          };
        })()
        """)
        print("search:", results)

        shot = await cmd("Page.captureScreenshot", {"format": "png"})
        with open("preview-search.png", "wb") as handle:
            handle.write(base64.b64decode(shot["data"]))

        await cmd("Input.dispatchKeyEvent", {
            "type": "rawKeyDown", "key": "Enter", "code": "Enter",
            "windowsVirtualKeyCode": 13, "nativeVirtualKeyCode": 13,
        })
        await asyncio.sleep(1.5)
        after = await evaluate(r"""
        (() => ({
          overlay: !!document.querySelector('[data-result]'),
          path: document.querySelector('[data-status]').innerText.split('\n').pop(),
        }))()
        """)
        print("after enter:", after)

        nav_exe = await evaluate(r"""
        (async () => {
          HELPERS
          const root = Array.from(pane().querySelectorAll('button')).find((b) => b.textContent.trim() === 'C:');
          root.dispatchEvent(new MouseEvent('click', { bubbles: true }));
          await wait(() => pane().querySelector('[data-row="Windows"]'));
          await openRow('Windows');
          await openRow('System32');
          return pane().querySelector('[data-status]').innerText.split('\n').pop();
        })()
        """)
        print("system32:", nav_exe)

        final = await evaluate(r"""
        (async () => {
          HELPERS
          await clickRow('notepad.exe');
          await wait(() => document.querySelector('aside.w-\\[320px\\]')?.innerText.includes('Version') || document.querySelector('aside.w-\\[320px\\]')?.innerText.includes('版本'));
          return document.querySelector('aside.w-\\[320px\\]').innerText.replace(/\n/g, ' | ').slice(0, 240);
        })()
        """)
        print("exe preview:", final)

        shot = await cmd("Page.captureScreenshot", {"format": "png"})
        with open("preview-exe.png", "wb") as handle:
            handle.write(base64.b64decode(shot["data"]))


asyncio.run(main())
