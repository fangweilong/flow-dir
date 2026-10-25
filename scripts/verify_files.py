"""Exercise selection + keyboard on the file list, then capture a screenshot."""
import asyncio
import base64
import json
import sys
import urllib.request

import websockets


def page_target() -> str:
    targets = json.load(urllib.request.urlopen("http://127.0.0.1:9333/json/list", timeout=10))
    return next(t for t in targets if t.get("type") == "page")["webSocketDebuggerUrl"]


SET_FILTER = r"""
(() => {
  const input = document.querySelector('input[placeholder="筛选当前目录"]');
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
  setter.call(input, '.exe');
  input.dispatchEvent(new Event('input', { bubbles: true }));
  return true;
})()
"""

READ_STATE = r"""
(() => {
  const row = document.querySelector('[data-row]');
  const rect = row ? row.getBoundingClientRect() : null;
  const selected = Array.from(document.querySelectorAll('[data-row]'))
    .filter((el) => el.className.includes('bg-blue-100'))
    .map((el) => el.dataset.row);
  return {
    rect: rect ? { x: rect.x + 120, y: rect.y + rect.height / 2 } : null,
    firstRows: Array.from(document.querySelectorAll('[data-row]')).slice(0, 6)
      .map((el) => el.innerText.replace(/\n/g, ' | ')),
    selected,
    footer: document.querySelector('footer').innerText.replace(/\n/g, ' | '),
  };
})()
"""


async def main() -> None:
    async with websockets.connect(page_target(), max_size=32 * 1024 * 1024) as ws:
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

        async def evaluate(expression: str) -> dict:
            result = await cmd(
                "Runtime.evaluate",
                {"expression": expression, "awaitPromise": True, "returnByValue": True},
            )
            if "exceptionDetails" in result:
                raise RuntimeError(json.dumps(result["exceptionDetails"], ensure_ascii=False))
            return result["result"].get("value")

        await cmd("Page.enable")
        await evaluate(SET_FILTER)
        await asyncio.sleep(0.6)

        state = await evaluate(READ_STATE)
        print("before:", json.dumps(state, ensure_ascii=False))

        x, y = state["rect"]["x"], state["rect"]["y"]
        await cmd("Input.dispatchMouseEvent", {"type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1})
        await cmd("Input.dispatchMouseEvent", {"type": "mouseReleased", "x": x, "y": y, "button": "left", "clickCount": 1})
        await asyncio.sleep(0.2)

        await cmd("Input.dispatchKeyEvent", {
            "type": "rawKeyDown", "key": "ArrowDown", "code": "ArrowDown",
            "windowsVirtualKeyCode": 40, "nativeVirtualKeyCode": 40,
        })
        await cmd("Input.dispatchKeyEvent", {
            "type": "keyUp", "key": "ArrowDown", "code": "ArrowDown",
            "windowsVirtualKeyCode": 40, "nativeVirtualKeyCode": 40,
        })
        await asyncio.sleep(0.2)

        after = await evaluate(READ_STATE)
        print("after:", json.dumps(after, ensure_ascii=False))

        shot = await cmd("Page.captureScreenshot", {"format": "png"})
        out = sys.argv[1] if len(sys.argv) > 1 else "shot3.png"
        with open(out, "wb") as handle:
            handle.write(base64.b64decode(shot["data"]))
        print("screenshot:", out)


asyncio.run(main())
