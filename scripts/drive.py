"""Drive the running FlowDir window over the WebView2 CDP endpoint.

Requires the app window to be built with
additionalBrowserArgs: "--remote-debugging-port=9333" (debug builds only).
"""
import asyncio
import base64
import json
import sys
import urllib.request

import websockets

DEBUG_URL = "http://127.0.0.1:9333/json/list"


def page_target():
    targets = json.load(urllib.request.urlopen(DEBUG_URL, timeout=10))
    pages = [t for t in targets if t.get("type") == "page"]
    if not pages:
        raise SystemExit("no page target")
    return pages[0]["webSocketDebuggerUrl"]


NAV_JS = r"""
(async () => {
  const raf = () => new Promise((r) => requestAnimationFrame(r));
  const rows = () => document.querySelectorAll('[data-row]').length;
  const setFilter = (value) => {
    const input = document.querySelector('input[placeholder="筛选当前目录"]');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(input, value);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  };
  const settle = async () => {
    let last = -1, stable = 0;
    for (let i = 0; i < 2000; i++) {
      await raf();
      const n = rows();
      const busy = document.body.innerText.includes('扫描中') || document.body.innerText.includes('正在读取目录');
      if (n === last && !busy) { if (++stable > 10) return; } else { stable = 0; }
      last = n;
    }
  };
  const crumbLabels = () => Array.from(document.querySelectorAll('header button')).map((b) => b.textContent.trim());
  const clickCrumb = async (label) => {
    const btn = Array.from(document.querySelectorAll('header button')).find((b) => b.textContent.trim() === label);
    if (!btn) throw new Error('crumb not found: ' + label);
    btn.click();
    await settle();
  };
  const nav = async (name) => {
    setFilter(name);
    for (let i = 0; i < 60 && !document.querySelector('[data-row="' + name + '"]'); i++) await raf();
    const row = document.querySelector('[data-row="' + name + '"]');
    if (!row) throw new Error('row not found: ' + name);
    const t0 = performance.now();
    row.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    setFilter('');
    while (!crumbLabels().includes(name)) {
      if (performance.now() - t0 > 20000) throw new Error('timeout waiting crumb: ' + name);
      await raf();
    }
    let first = null;
    while (first === null) {
      if (rows() > 0) first = performance.now() - t0;
      else if (performance.now() - t0 > 20000) throw new Error('timeout waiting rows');
      await raf();
    }
    await settle();
    const done = performance.now() - t0;
    const footer = document.querySelector('footer').innerText.split('\n')[0];
    return { name, firstMs: Math.round(first), doneMs: Math.round(done), footer };
  };
  const start = document.querySelector('header').innerText.replace(/\n/g, ' ');
  await clickCrumb('C:');
  const a = await nav('Windows');
  const b = await nav('System32');
  return { start, windows: a, system32: b, crumb: document.querySelector('header').innerText.split('\n').pop() };
})()
"""


async def main() -> None:
    ws_url = page_target()
    async with websockets.connect(ws_url, max_size=32 * 1024 * 1024) as ws:
        counter = 0

        async def cmd(method: str, params: dict | None = None) -> dict:
            nonlocal counter
            counter += 1
            await ws.send(json.dumps({"id": counter, "method": method, "params": params or {}}))
            while True:
                msg = json.loads(await ws.recv())
                if msg.get("id") == counter:
                    if "error" in msg:
                        raise RuntimeError(msg["error"])
                    return msg.get("result", {})

        await cmd("Page.enable")
        result = await cmd(
            "Runtime.evaluate",
            {"expression": NAV_JS, "awaitPromise": True, "returnByValue": True},
        )
        if "exceptionDetails" in result:
            print(json.dumps(result["exceptionDetails"], ensure_ascii=False, indent=2))
        value = result.get("result", {}).get("value")
        print(json.dumps(value, ensure_ascii=False, indent=2))

        shot = await cmd("Page.captureScreenshot", {"format": "png"})
        out = sys.argv[1] if len(sys.argv) > 1 else "shot2.png"
        with open(out, "wb") as handle:
            handle.write(base64.b64decode(shot["data"]))
        print("screenshot:", out)


asyncio.run(main())
