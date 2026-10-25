"""Verify P4: IFileOperation copy/cancel/rename/delete/newdir, custom menus, about page."""
import asyncio
import base64
import json
import os
import urllib.request

import websockets

ROOT = os.path.join(os.environ["USERPROFILE"], "flowdir-ops")

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
    return row;
  };
  const statusPath = () => pane().querySelector('[data-status]').innerText.split('\n').pop();
  const openRow = async (name) => {
    const before = statusPath();
    await clickRow(name);
    setFilter(name);
    await wait(() => pane().querySelector('[data-row="' + name + '"]'));
    const row = pane().querySelector('[data-row="' + name + '"]');
    row.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    await raf();
    await wait(() => statusPath() !== before);
    await wait(() => !pane().querySelector('[data-status]').innerText.includes('扫描中'));
    await raf();
  };
  const key = (k, opts = {}) => {
    listEl().focus();
    listEl().dispatchEvent(new KeyboardEvent('keydown', Object.assign({ key: k, bubbles: true, cancelable: true }, opts)));
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

        print("nav src:", await run(r"""
          await go(['flowdir-ops', 'src']);
          await raf();
          return { path: pane().querySelector('[data-status]').innerText.split('\n').pop(), rows: rows() };
        """))

        print("copy+paste staged:", await run(r"""
          await clickRow('big.bin');
          key('c', { ctrlKey: true });
          await raf();
          await go(['flowdir-ops', 'dst']);
          await raf();
          key('v', { ctrlKey: true });
          await wait(() => document.querySelector('[data-op]'));
          const card = document.querySelector('[data-op]');
          return { card: card.innerText.split('\n').slice(0, 2) };
        """))
        await asyncio.sleep(0.2)
        await shot("p4-progress-copy.png")
        print("cancel:", await run(r"""
          const card = document.querySelector('[data-op]');
          const before = card ? card.innerText.split('\n')[0] : null;
          const btn = document.querySelector('[data-op-cancel]');
          if (btn) btn.click();
          await wait(() => !document.querySelector('[data-op]'), 30000);
          await raf(); await raf();
          return { before, rows: rows(), toast: document.querySelector('.fixed.bottom-4')?.innerText ?? null };
        """))
        dest = os.path.join(ROOT, "dst", "big.bin")
        print("dst big.bin after cancel:", os.path.getsize(dest) if os.path.exists(dest) else None)

        print("rename:", await run(r"""
          await go(['flowdir-ops', 'src']);
          await clickRow('note.txt');
          key('F2');
          await wait(() => document.querySelector('[data-prompt-input]'));
          const input = document.querySelector('[data-prompt-input]');
          const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
          setter.call(input, 'note2.txt');
          input.dispatchEvent(new Event('input', { bubbles: true }));
          input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
          await wait(() => rows().includes('note2.txt'), 20000);
          return rows();
        """))

        print("delete:", await run(r"""
          await clickRow('note2.txt');
          key('Delete');
          await wait(() => !rows().includes('note2.txt'), 20000);
          return rows();
        """))

        print("newdir:", await run(r"""
          const list = listEl();
          const rect = list.getBoundingClientRect();
          list.dispatchEvent(new MouseEvent('contextmenu', {
            bubbles: true, cancelable: true, clientX: rect.left + 40, clientY: rect.bottom - 60,
          }));
          await wait(() => document.querySelector('[data-context-menu]'));
          const menu = document.querySelector('[data-context-menu]');
          const items = Array.from(menu.querySelectorAll('button')).map((b) => b.innerText.trim());
          Array.from(menu.querySelectorAll('button')).find((b) => /新建文件夹|New folder/.test(b.innerText)).click();
          await wait(() => document.querySelector('[data-prompt-input]'));
          const input = document.querySelector('[data-prompt-input]');
          const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
          setter.call(input, 'created');
          input.dispatchEvent(new Event('input', { bubbles: true }));
          input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
          await wait(() => rows().includes('created'), 20000);
          return { items, rows: rows() };
        """))

        print("about:", await evaluate(r"""
        (async () => {
          const raf = () => new Promise((r) => requestAnimationFrame(r));
          const gear = Array.from(document.querySelectorAll('header button')).pop();
          gear.click();
          await raf(); await raf(); await raf();
          const modal = document.querySelector('.fixed.inset-0.z-50');
          if (!modal) return { error: 'no modal' };
          const rows = Array.from(modal.querySelectorAll('.space-y-0\\.5 > div')).map((row) => row.innerText.replace(/\n/g, ' | '));
          return { count: rows.length, sample: rows.slice(0, 5), tauri: rows.find((r) => r.startsWith('tauri')) ?? null };
        })()
        """))
        await shot("p4-about.png")


asyncio.run(main())
