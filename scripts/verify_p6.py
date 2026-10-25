"""Verify settings toggles, address bar, history keys, properties dialog and provider switch."""
import asyncio
import json
import os
import subprocess
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
  const panes = () => Array.from(document.querySelectorAll('[data-pane-id]'));
  const listEl = () => document.activeElement?.matches('div[tabindex="0"]')
    ? document.activeElement
    : pane().querySelector('div[tabindex="0"]');
  const setValue = (input, value) => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(input, value);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  };
  const filterEl = () => pane().querySelector('input[placeholder="筛选"], input[placeholder="Filter"]');
  const rows = () => Array.from(pane().querySelectorAll('[data-row]')).map((r) => r.dataset.row);
  const statusPath = () => pane().querySelector('[data-status]').innerText.split('\n').pop();
  const clickRow = async (name) => {
    setValue(filterEl(), '');
    await raf();
    setValue(filterEl(), name);
    await wait(() => pane().querySelector('[data-row="' + name + '"]'));
    const row = pane().querySelector('[data-row="' + name + '"]');
    const r = row.getBoundingClientRect();
    row.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, button: 0, clientX: r.x + 30, clientY: r.y + 10 }));
    setValue(filterEl(), '');
    await raf();
    return name;
  };
  const key = (k, opts = {}) => {
    const el = pane().querySelector('div[tabindex="0"]');
    el.focus();
    const event = new KeyboardEvent('keydown', Object.assign({ key: k, bubbles: true, cancelable: true }, opts));
    el.dispatchEvent(event);
  };
  const goHome = async () => {
    setValue(filterEl(), '');
    const btn = Array.from(document.querySelectorAll('aside button')).find((b) => /主目录|Home/.test(b.innerText));
    if (btn) {
      btn.dispatchEvent(new MouseEvent('click', { bubbles: true }));
      await raf(); await raf(); await raf();
    }
  };
  const openSettings = async () => {
    Array.from(document.querySelectorAll('header button')).find((b) => b.title === '设置' || b.title === 'Settings').click();
    await wait(() => document.querySelector('[data-toggle]'));
  };
  const toggleState = (label) => {
    const button = Array.from(document.querySelectorAll('[data-toggle]')).find((b) => b.dataset.toggle === label);
    if (!button) throw new Error('no toggle ' + label);
    return button.getAttribute('aria-checked') === 'true';
  };
  const setToggle = (label, wanted) => {
    if (toggleState(label) !== wanted) {
      Array.from(document.querySelectorAll('[data-toggle]')).find((b) => b.dataset.toggle === label).click();
    }
  };
  const closeSettings = async () => {
    const modal = document.querySelector('.fixed.inset-0.z-50');
    modal.querySelector('button').click();
    await raf(); await raf();
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

        await cmd("Page.enable")

        print("toggles:", await run(r"""
          await openSettings();
          const labels = Array.from(document.querySelectorAll('[data-toggle]')).map((b) => b.dataset.toggle);
          return { labels, everythingHint: Array.from(document.querySelectorAll('div')).map(d=>d.innerText).find(t=>t.startsWith('Everything ')) ?? null };
        """))

        # show hidden for new panes, then create a 2-pane layout and inspect the new pane
        print("showHidden:", await run(r"""
          await goHome();
          await openSettings();
          setToggle('新窗格显示隐藏项', true);
          await raf(); await raf();
          await closeSettings();
          Array.from(document.querySelectorAll('header button')).find((b) => b.title === '左右双窗格').click();
          await wait(() => panes().length === 2);
          await wait(() => panes()[1].querySelector('[data-row]'));
          const hiddenRows = Array.from(panes()[1].querySelectorAll('[data-row]')).map((r) => r.dataset.row).filter((n) => n.startsWith('.'));
          const hiddenButton = panes()[1].querySelector('button[title*="隐藏"]');
          return { count: panes().length, hiddenRows: hiddenRows.slice(0, 4), hiddenOn: hiddenButton?.className.includes('text-blue-500') ?? null };
        """))

        # collapse back to a single pane
        await run(r"""
          Array.from(document.querySelectorAll('header button')).find((b) => b.title === '单窗格').click();
          await wait(() => panes().length === 1);
          return 'ok';
        """)

        # address bar: Ctrl+L then navigate to C:\Windows (String.raw avoids escapes)
        print("address:", await run(r"""
          await goHome();
          await raf();
          key('l', { ctrlKey: true });
          await wait(() => document.querySelector('[data-address-input]'));
          const input = document.querySelector('[data-address-input]');
          setValue(input, String.raw`C:\Windows`);
          input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
          await wait(() => statusPath().replace(/\\/g, '\\') === 'C:\\Windows' && !pane().querySelector('[data-address-input]'), 20000);
          return { path: statusPath(), rows: rows().length };
        """))

        # Alt+Left goes back to the previous folder
        print("alt-left:", await run(r"""
          const before = statusPath();
          key('ArrowLeft', { altKey: true });
          await wait(() => statusPath() !== before, 20000);
          return statusPath();
        """))

        # Alt+Enter opens the property sheet
        print("properties:", await run(r"""
          key('l', { ctrlKey: true });
          await wait(() => document.querySelector('[data-address-input]'));
          const input = document.querySelector('[data-address-input]');
          setValue(input, String.raw`C:\Windows`);
          input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
          await wait(() => /Windows\\?$/.test(statusPath()) && !pane().querySelector('[data-address-input]'), 20000);
          await clickRow('notepad.exe');
          key('Enter', { altKey: true });
          return 'sent';
        """))
        await asyncio.sleep(2)
        probe = subprocess.run(
            ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "scripts/props_probe.ps1"],
            capture_output=True,
            text=True,
        )
        print("property window:", probe.stdout.strip() or probe.stderr.strip()[:200])
        subprocess.run(
            ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "scripts/props_close.ps1"],
            capture_output=True,
            text=True,
        )

        # Everything toggle off -> search must fall back to the built-in scanner
        print("provider off:", await run(r"""
          await openSettings();
          setToggle('搜索优先使用 Everything', false);
          await raf(); await raf();
          await closeSettings();
          key('f', { ctrlKey: true });
          await wait(() => document.querySelector('.fixed.inset-0.z-50 input'));
          const input = document.querySelector('.fixed.inset-0.z-50 input');
          setValue(input, 'notepad');
          await wait(() => /内置扫描|Built-in scan/.test(document.body.innerText), 20000);
          const badge = Array.from(document.querySelectorAll('span')).map((s) => s.innerText).find((t) => t === '内置扫描' || t === 'Built-in scan');
          document.querySelector('.fixed.inset-0.z-50').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
          return { badge, results: document.body.innerText.match(/(\\d+) 个结果/)?.[1] ?? null };
        """))


asyncio.run(main())
