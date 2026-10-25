"""Verify multi-pane layout, tabs and drag-to-split over CDP."""
import asyncio
import base64
import json
import sys
import urllib.request

import websockets

STATE_JS = r"""
(() => {
  const panes = Array.from(document.querySelectorAll('[data-pane-id]'));
  return panes.map((p) => ({
    id: p.dataset.paneId,
    tabs: Array.from(p.querySelectorAll('[data-tab]')).map((t) => ({
      id: t.dataset.tab,
      active: t.dataset.tabActive === '1',
      label: t.innerText.replace(/\n/g, ' ').trim(),
    })),
    rows: p.querySelectorAll('[data-row]').length,
    footer: p.querySelector('[data-status]')?.innerText.replace(/\n/g, ' | ') ?? '',
    rect: (() => {
      const r = p.getBoundingClientRect();
      return { x: r.x, y: r.y, w: r.width, h: r.height };
    })(),
  }));
})()
"""


def page_target() -> str:
    targets = json.load(urllib.request.urlopen("http://127.0.0.1:9333/json/list", timeout=10))
    return next(t for t in targets if t.get("type") == "page")["webSocketDebuggerUrl"]


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

        async def evaluate(expression: str) -> object:
            result = await cmd(
                "Runtime.evaluate",
                {"expression": expression, "awaitPromise": True, "returnByValue": True},
            )
            if "exceptionDetails" in result:
                raise RuntimeError(json.dumps(result["exceptionDetails"], ensure_ascii=False))
            return result["result"].get("value")

        async def state() -> list:
            return await evaluate(STATE_JS)

        async def click_title(title: str) -> None:
            await evaluate(f"document.querySelector('[title=\"{title}\"]').click()")

        async def mouse(kind: str, x: float, y: float) -> None:
            await cmd("Input.dispatchMouseEvent", {
                "type": kind, "x": x, "y": y, "button": "left", "clickCount": 1})

        async def drag(src: tuple[float, float], dst: tuple[float, float]) -> None:
            await mouse("mousePressed", *src)
            for i in range(1, 9):
                await mouse("mouseMoved", src[0] + (dst[0] - src[0]) * i / 8, src[1] + (dst[1] - src[1]) * i / 8)
                await asyncio.sleep(0.03)
            await mouse("mouseReleased", *dst)
            await asyncio.sleep(0.8)

        await cmd("Page.enable")
        await asyncio.sleep(0.5)
        print("initial:", json.dumps(await state(), ensure_ascii=False))

        await click_title("四窗格")
        await asyncio.sleep(1.5)
        quad = await state()
        print("quad:", json.dumps([{ "tabs": len(p["tabs"]), "rows": p["rows"], "footer": p["footer"]} for p in quad], ensure_ascii=False))

        # add a second tab in pane 0, then drag it onto pane 1 center
        await evaluate("document.querySelectorAll('[title=\"新建标签\"]')[0].click()")
        await asyncio.sleep(1.0)
        after_new = await state()
        print("after new tab:", json.dumps([{"tabs": len(p["tabs"]), "rows": p["rows"]} for p in after_new], ensure_ascii=False))

        tabs = await evaluate("""
        (() => {
          const pane = document.querySelectorAll('[data-pane-id]')[0];
          const el = pane.querySelector('[data-tab-active="1"]');
          const r = el.getBoundingClientRect();
          return { x: r.x + r.width / 2, y: r.y + r.height / 2 };
        })()
        """)
        target_pane = (await state())[1]
        dst = (target_pane["rect"]["x"] + target_pane["rect"]["w"] / 2, target_pane["rect"]["y"] + target_pane["rect"]["h"] * 0.6)
        await drag((tabs["x"], tabs["y"]), dst)
        after_move = await state()
        print("after move:", json.dumps([{"tabs": len(p["tabs"]), "rows": p["rows"]} for p in after_move], ensure_ascii=False))

        # drag a tab to the right edge of pane 2 to create a split
        source = await evaluate("""
        (() => {
          const panes = document.querySelectorAll('[data-pane-id]');
          const pane = panes[2] || panes[0];
          const el = pane.querySelector('[data-tab]');
          const r = el.getBoundingClientRect();
          const pr = pane.getBoundingClientRect();
          return { x: r.x + r.width / 2, y: r.y + r.height / 2, edgeX: pr.right - 30, edgeY: pr.y + pr.height * 0.6 };
        })()
        """)
        await drag((source["x"], source["y"]), (source["edgeX"], source["edgeY"]))
        after_split = await state()
        print("after split:", json.dumps([{"tabs": len(p["tabs"]), "rows": p["rows"]} for p in after_split], ensure_ascii=False))
        print("pane count:", len(after_split))

        shot = await cmd("Page.captureScreenshot", {"format": "png"})
        out = sys.argv[1] if len(sys.argv) > 1 else "panes.png"
        with open(out, "wb") as handle:
            handle.write(base64.b64decode(shot["data"]))
        print("screenshot:", out)


asyncio.run(main())
