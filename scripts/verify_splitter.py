"""Verify last-tab replacement and splitter resizing over CDP."""
import asyncio
import json
import urllib.request

import websockets

JS = r"""
(async () => {
  const raf = () => new Promise((r) => requestAnimationFrame(r));
  const wait = async (cond, ms = 8000) => {
    const t0 = performance.now();
    while (!cond()) {
      if (performance.now() - t0 > ms) throw new Error('timeout');
      await raf();
    }
  };
  const count = (sel) => document.querySelectorAll(sel).length;

  let n = count('[data-tab]');
  while (n > 1) {
    const before = n;
    document.querySelectorAll('[data-tab]')[0].querySelector('button')
      .dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await wait(() => count('[data-tab]') === before - 1);
    n = before - 1;
  }
  const beforeLastClose = count('[data-tab]');
  document.querySelectorAll('[data-tab]')[0].querySelector('button')
    .dispatchEvent(new MouseEvent('click', { bubbles: true }));
  await wait(() => count('[data-tab]') === 1 && count('[data-row]') > 0);
  const afterLastClose = { tabs: count('[data-tab]'), rows: count('[data-row]') };

  document.querySelector('[title="左右双窗格"]').click();
  await wait(() => count('[data-pane-id]') === 2);
  await raf();
  const widthsBefore = Array.from(document.querySelectorAll('[data-pane-id]')).map((p) => Math.round(p.getBoundingClientRect().width));

  const sep = document.querySelector('.cursor-col-resize');
  const r = sep.getBoundingClientRect();
  sep.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, clientX: r.x + 1, clientY: r.y + 5, button: 0 }));
  await raf();
  window.dispatchEvent(new PointerEvent('pointermove', { clientX: r.x + 150, clientY: r.y + 5 }));
  await raf();
  window.dispatchEvent(new PointerEvent('pointerup', {}));
  await raf();
  const widthsAfter = Array.from(document.querySelectorAll('[data-pane-id]')).map((p) => Math.round(p.getBoundingClientRect().width));

  return { beforeLastClose, afterLastClose, widthsBefore, widthsAfter, panes: count('[data-pane-id]') };
})()
"""


async def main() -> None:
    targets = json.load(urllib.request.urlopen("http://127.0.0.1:9333/json/list", timeout=10))
    ws_url = next(t for t in targets if t.get("type") == "page")["webSocketDebuggerUrl"]
    async with websockets.connect(ws_url, max_size=32 * 1024 * 1024) as ws:
        await ws.send(json.dumps({"id": 1, "method": "Runtime.evaluate", "params": {
            "expression": JS, "awaitPromise": True, "returnByValue": True}}))
        while True:
            msg = json.loads(await ws.recv())
            if msg.get("id") == 1:
                result = msg.get("result", {})
                if "exceptionDetails" in result:
                    raise SystemExit(json.dumps(result["exceptionDetails"], ensure_ascii=False))
                print(json.dumps(result["result"].get("value"), ensure_ascii=False, indent=2))
                return


asyncio.run(main())
