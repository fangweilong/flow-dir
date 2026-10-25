"""Verify shell icons, video first frame and text encodings."""
import asyncio
import base64
import json
import urllib.request

import websockets

HELPERS = r"""
  const raf = () => new Promise((r) => requestAnimationFrame(r));
  const wait = async (cond, ms = 30000) => {
    const t0 = performance.now();
    while (!cond()) {
      if (performance.now() - t0 > ms) throw new Error('timeout');
      await raf();
    }
  };
  const pane = () => document.querySelector('[data-pane-id]');
  const listEl = () => pane().querySelector('div[tabindex="0"]');
  const filterEl = () => pane().querySelector('input[placeholder="筛选"], input[placeholder="Filter"]');
  const setFilter = (value) => {
    const input = filterEl();
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(input, value);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  };
  const rows = () => Array.from(pane().querySelectorAll('[data-row]')).map((r) => r.dataset.row);
  const statusPath = () => pane().querySelector('[data-status]').innerText.split('\n').pop();
  const clickRow = async (name) => {
    setFilter('');
    await raf();
    setFilter(name);
    await wait(() => pane().querySelector('[data-row="' + name + '"]'));
    const row = pane().querySelector('[data-row="' + name + '"]');
    const r = row.getBoundingClientRect();
    row.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, button: 0, clientX: r.x + 30, clientY: r.y + 10 }));
    setFilter('');
    await raf();
  };
  const openRow = async (name) => {
    const before = statusPath();
    await clickRow(name);
    setFilter(name);
    await wait(() => pane().querySelector('[data-row="' + name + '"]'));
    const row = pane().querySelector('[data-row="' + name + '"]');
    row.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    setFilter('');
    await raf();
    await wait(() => statusPath() !== before);
    await wait(() => !pane().querySelector('[data-status]').innerText.includes('扫描中'));
    await raf();
  };
  const home = async () => {
    setFilter('');
    const btn = Array.from(document.querySelectorAll('aside button')).find((b) => /主目录|Home/.test(b.innerText));
    if (btn) {
      btn.dispatchEvent(new MouseEvent('click', { bubbles: true }));
      await raf(); await raf(); await raf();
    }
  };
  const go = async (segments) => {
    await home();
    for (const seg of segments) await openRow(seg);
  };
  const panelText = () => document.querySelector('aside.w-\\[320px\\]')?.innerText ?? '';
"""


def wrap(body: str) -> str:
    return "(async () => {\n" + HELPERS + "\n" + body + "\n})()"


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
            result = await cmd(
                "Runtime.evaluate",
                {"expression": expression, "awaitPromise": True, "returnByValue": True},
            )
            if "exceptionDetails" in result:
                raise RuntimeError(json.dumps(result["exceptionDetails"], ensure_ascii=False)[:900])
            return result["result"].get("value")

        async def run(body):
            return await evaluate(wrap(body))

        async def shot(name):
            data = await cmd("Page.captureScreenshot", {"format": "png"})
            with open(name, "wb") as handle:
                handle.write(base64.b64decode(data["data"]))

        await cmd("Page.enable")

        print("icons:", await run(r"""
          await go(['flowdir-preview2']);
          await raf(); await raf();
          await wait(() => rows().length >= 4);
          const imgs = Array.from(pane().querySelectorAll('img[src*="fd.localhost/icon"]'));
          await wait(() => imgs.every((img) => img.complete && img.naturalWidth > 0), 20000);
          const probe = await fetch(imgs[0].src);
          const bytes = new Uint8Array(await probe.arrayBuffer());
          return {
            count: imgs.length,
            sizes: Array.from(new Set(imgs.map((i) => i.naturalWidth + 'x' + i.naturalHeight))),
            status: probe.status,
            type: probe.headers.get('content-type'),
            png: bytes[0] === 0x89 && bytes[1] === 0x50 && bytes[2] === 0x4e && bytes[3] === 0x47,
            folderIcon: imgs[0].src.includes('flowdir-preview2'),
          };
        """))

        async def preview_of(name, kind_expect=None):
            return await run(r"""
              await clickRow('""" + name + r"""');
              await wait(() => {
                const panel = document.querySelector('aside.w-\\[320px\\]');
                return panel && panel.innerText.includes('""" + name + r"""') && !panel.innerText.includes('正在生成预览');
              }, 20000);
              await raf(); await raf();
              const panel = document.querySelector('aside.w-\\[320px\\]');
              const img = panel.querySelector('img');
              if (img) await wait(() => img.complete, 15000);
              return {
                text: panel.innerText.replace(/\n/g, ' | ').slice(0, 220),
                img: img ? img.src : null,
                w: img ? img.naturalWidth : 0,
                h: img ? img.naturalHeight : 0,
              };
            """)

        print("utf16:", await preview_of("utf16.txt"))
        print("gbk:", await preview_of("gbk.txt"))
        print("utf8:", await preview_of("utf8.txt"))
        print("video:", await preview_of("demo.mp4"))
        await shot("p5-video.png")


asyncio.run(main())
