"""Verify layout collapse and tab closing over CDP."""
import asyncio
import json
import sys
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

  document.querySelector('[title="单窗格"]').click();
  await wait(() => count('[data-pane-id]') === 1);
  const tabsAfterLayout = count('[data-tab]');
  const labels = Array.from(document.querySelectorAll('[data-tab]')).map((t) => t.title);

  const before = count('[data-tab]');
  for (let i = 0; i < 2; i++) {
    const button = document.querySelectorAll('[data-tab]')[0].querySelector('button');
    button.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await wait(() => count('[data-tab]') === before - i - 1);
  }
  return {
    panes: count('[data-pane-id]'),
    tabsAfterLayout,
    labels,
    tabsAfterClose: count('[data-tab]'),
    rows: count('[data-row]'),
  };
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
