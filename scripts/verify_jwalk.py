"""Verify the built-in (jwalk) search fallback and capture preview screenshots."""
import asyncio
import base64
import json
import urllib.request

import websockets

JS = r"""
(async () => {
  const raf = () => new Promise((r) => requestAnimationFrame(r));
  const wait = async (cond, ms = 20000) => {
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
  const clickRow = async (name) => {
    setFilter(name);
    await wait(() => pane().querySelector('[data-row="' + name + '"]'));
    const row = pane().querySelector('[data-row="' + name + '"]');
    const r = row.getBoundingClientRect();
    row.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, button: 0, clientX: r.x + 30, clientY: r.y + 10 }));
    setFilter('');
    await raf();
  };

  const home = Array.from(document.querySelectorAll('aside button')).find((b) => /主目录|Home/.test(b.innerText));
  home.dispatchEvent(new MouseEvent('click', { bubbles: true }));
  await raf();
  await raf();
  setFilter('flowdir-preview');
  await wait(() => pane().querySelector('[data-row="flowdir-preview"]'));
  pane().querySelector('[data-row="flowdir-preview"]').dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
  setFilter('');
  await wait(() => pane().querySelector('[data-status]').innerText.includes('项'));
  await raf();
  await clickRow('demo.png');
  await wait(() => {
    const panel = document.querySelector('aside.w-\\[320px\\]');
    return panel && panel.innerText.includes('demo.png') && !panel.innerText.includes('正在生成预览');
  });
  return { path: pane().querySelector('[data-status]').innerText.split('\n').pop() };
})()
"""

SEARCH_JS = r"""
(async () => {
  const raf = () => new Promise((r) => requestAnimationFrame(r));
  const t0 = performance.now();
  for (let i = 0; i < 2000; i++) {
    await raf();
    const badge = document.querySelector('.fixed.inset-0 .rounded.bg-surface-3');
    if (badge && badge.innerText) {
      return { badge: badge.innerText, count: document.querySelectorAll('[data-result]').length, ms: Math.round(performance.now() - t0) };
    }
  }
  return { badge: null, count: 0, ms: Math.round(performance.now() - t0) };
})()
"""


async def main() -> None:
    targets = json.load(urllib.request.urlopen("http://127.0.0.1:9333/json/list", timeout=10))
    ws_url = next(t for t in targets if t.get("type") == "page")["webSocketDebuggerUrl"]
    async with websockets.connect(ws_url, max_size=32 * 1024 * 1024) as ws:
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
            result = await cmd("Runtime.evaluate", {"expression": expression, "awaitPromise": True, "returnByValue": True})
            if "exceptionDetails" in result:
                raise RuntimeError(json.dumps(result["exceptionDetails"], ensure_ascii=False)[:600])
            return result["result"].get("value")

        await cmd("Page.enable")
        await asyncio.sleep(0.5)
        print("preview:", await evaluate(JS))
        shot = await cmd("Page.captureScreenshot", {"format": "png"})
        with open("preview-image.png", "wb") as handle:
            handle.write(base64.b64decode(shot["data"]))

        await cmd("Input.dispatchKeyEvent", {
            "type": "rawKeyDown", "key": "f", "code": "KeyF",
            "windowsVirtualKeyCode": 70, "nativeVirtualKeyCode": 70, "modifiers": 2,
        })
        await cmd("Input.dispatchKeyEvent", {
            "type": "keyUp", "key": "f", "code": "KeyF",
            "windowsVirtualKeyCode": 70, "nativeVirtualKeyCode": 70, "modifiers": 2,
        })
        await asyncio.sleep(0.4)
        await cmd("Input.insertText", {"text": "demo"})
        await asyncio.sleep(3.0)
        print("jwalk search:", await evaluate(SEARCH_JS))

        shot = await cmd("Page.captureScreenshot", {"format": "png"})
        with open("search-jwalk.png", "wb") as handle:
            handle.write(base64.b64decode(shot["data"]))


asyncio.run(main())
