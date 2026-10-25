"""Set up a clean four-pane view and capture it."""
import asyncio
import base64
import json
import sys
import urllib.request

import websockets

JS = r"""
(async () => {
  const raf = () => new Promise((r) => requestAnimationFrame(r));
  const wait = async (cond, ms = 15000) => {
    const t0 = performance.now();
    while (!cond()) {
      if (performance.now() - t0 > ms) throw new Error('timeout');
      await raf();
    }
  };
  const panes = () => Array.from(document.querySelectorAll('[data-pane-id]'));
  const setFilter = (pane, value) => {
    const input = pane.querySelector('input[placeholder="筛选"]');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(input, value);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  };
  const goRoot = async (pane) => {
    const crumb = Array.from(pane.querySelectorAll('button')).find((b) => b.textContent.trim() === 'C:');
    if (!crumb) throw new Error('no C: crumb');
    crumb.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await wait(() => pane.querySelector('[data-row="Windows"]'));
  };
  const openIn = async (index, name) => {
    const pane = panes()[index];
    pane.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
    await raf();
    await goRoot(pane);
    setFilter(pane, name);
    await wait(() => pane.querySelector('[data-row="' + name + '"]'));
    pane.querySelector('[data-row="' + name + '"]').dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    setFilter(pane, '');
    await wait(() => pane.querySelector('[data-status]').innerText.includes('项'));
    await raf();
  };
  const focus = async (index) => {
    panes()[index].dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
    await raf();
    await raf();
  };

  document.querySelector('[title="四窗格"]').click();
  await wait(() => panes().length === 4);
  await raf();

  await openIn(0, 'Windows');
  await openIn(1, 'Program Files');
  await focus(3);
  const dDrive = Array.from(document.querySelectorAll('aside button')).find((b) => b.innerText.trim().startsWith('D:'));
  if (dDrive) {
    dDrive.click();
    await wait(() => panes()[3].querySelector('[data-status]').innerText.includes('项'));
  }
  await focus(2);
  return { panes: panes().length, labels: panes().map((p) => p.querySelector('[data-status]').innerText.split('\n').pop()) };
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
                msg = json.loads(await ws.recv())
                if msg.get("id") == counter:
                    if "error" in msg:
                        raise RuntimeError(msg["error"])
                    return msg.get("result", {})

        await cmd("Page.enable")
        result = await cmd("Runtime.evaluate", {"expression": JS, "awaitPromise": True, "returnByValue": True})
        if "exceptionDetails" in result:
            raise SystemExit(json.dumps(result["exceptionDetails"], ensure_ascii=False))
        print(json.dumps(result["result"].get("value"), ensure_ascii=False))

        shot = await cmd("Page.captureScreenshot", {"format": "png"})
        out = sys.argv[1] if len(sys.argv) > 1 else "quad.png"
        with open(out, "wb") as handle:
            handle.write(base64.b64decode(shot["data"]))
        print("screenshot:", out)


asyncio.run(main())
