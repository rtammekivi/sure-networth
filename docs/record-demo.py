#!/usr/bin/env python3
"""Regenerate docs/demo.mp4 from docs/demo-data.json, from the repo root:

    nix develop .#demo -c python3 docs/record-demo.py
"""

import json
import shutil
import subprocess
import tempfile
import time
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

import websocket

ROOT = Path(__file__).resolve().parent.parent
W, H, FPS, PORT = 1100, 780, 12, 9335

OVERLAY = """
(() => {
  const cur = document.createElement('div');
  cur.id = 'demo-cursor';
  cur.innerHTML = '<svg width="22" height="26" viewBox="0 0 22 26"><path d="M2 2 L2 21 L7 16 L11 24 L14 22.5 L10 15 L17 15 Z" fill="#111" stroke="#fff" stroke-width="1.6" stroke-linejoin="round"/></svg>';
  Object.assign(cur.style, { position: 'fixed', left: '900px', top: '640px', zIndex: 99999,
    pointerEvents: 'none', transition: 'left .5s ease-in-out, top .5s ease-in-out' });
  const ring = document.createElement('div');
  Object.assign(ring.style, { position: 'fixed', width: '34px', height: '34px', margin: '-17px 0 0 -17px',
    borderRadius: '50%', border: '2px solid #2a78d6', zIndex: 99998, pointerEvents: 'none', opacity: 0,
    transform: 'scale(.4)', transition: 'transform .35s ease-out, opacity .35s ease-out' });
  ring.id = 'demo-ring';
  const cap = document.createElement('div');
  cap.id = 'demo-caption';
  Object.assign(cap.style, { position: 'fixed', left: '50%', bottom: '18px', transform: 'translateX(-50%)',
    background: 'rgba(11,11,11,.86)', color: '#fff', padding: '9px 18px', borderRadius: '999px',
    font: '600 15px system-ui, sans-serif', zIndex: 99999, pointerEvents: 'none',
    transition: 'opacity .25s ease', opacity: 0, whiteSpace: 'nowrap' });
  document.body.append(ring, cur, cap);
})();
"""


# The baked page has no Sure behind it, so the demo answers api/history itself:
# each past month-end is the demo data bent along a per-class path.
HISTORY = """
(() => {
  const paths = {
    Property: () => 1,
    Equities: (t, i) => 0.74 + 0.26 * t + 0.035 * Math.sin(i * 1.7),
    Pension: (t) => 0.86 + 0.14 * t,
    Bonds: (t) => 0.7 + 0.3 * Math.round(t * 3) / 3,
    Cash: (t, i) => 1.1 + 0.18 * Math.sin(i * 2.3) - 0.1 * t,
    Crypto: (t, i) => 0.62 + 0.38 * t + 0.16 * Math.sin(i * 1.1),
    Other: (t) => 1.18 - 0.18 * t,
  };
  CONFIG = {};
  window.fetch = async (url) => {
    const dates = new URL(url, location.href).searchParams.get("dates").split(",");
    const points = dates.map((as_of, i) => {
      const t = i / dates.length;
      const leaves = DATA.leaves.map((l) => ({ ...l, value: Math.round(l.value * (paths[l.class] || (() => 1))(t, i)) }));
      const liability_accounts = DATA.liability_accounts.map((l) => ({ ...l, value: Math.round(l.value * (1 + 0.035 * (1 - t))) }));
      return { ...DATA, as_of, leaves, liability_accounts };
    });
    return { ok: true, status: 200, json: async () => points };
  };
  document.getElementById("timeTab").hidden = false;
})();
"""


class Page:
    def __init__(self, ws_url):
        self.ws = websocket.create_connection(ws_url, timeout=30, suppress_origin=True)
        self.next_id = 0
        self.frames = []
        self.x, self.y = 900, 640

    def send(self, method, **params):
        self.next_id += 1
        self.ws.send(json.dumps({"id": self.next_id, "method": method, "params": params}))
        while True:
            msg = json.loads(self.ws.recv())
            if msg.get("id") == self.next_id:
                if "error" in msg:
                    raise RuntimeError(f"{method}: {msg['error']}")
                return msg.get("result", {})

    def js(self, expr):
        r = self.send("Runtime.evaluate", expression=expr, returnByValue=True, awaitPromise=True)
        if "exceptionDetails" in r:
            raise RuntimeError(f"{expr}: {r['exceptionDetails']}")
        return r["result"].get("value")

    def hold(self, seconds):
        end = time.monotonic() + seconds
        while True:
            start = time.monotonic()
            shot = self.send("Page.captureScreenshot", format="png")
            self.frames.append(shot["data"])
            if start >= end:
                return
            time.sleep(max(0.0, 1 / FPS - (time.monotonic() - start)))

    def center(self, selector_js):
        r = self.js(f"""(() => {{
            const el = {selector_js};
            if (!el) return null;
            const b = el.getBoundingClientRect();
            return [b.left + b.width / 2, b.top + b.height / 2];
        }})()""")
        if r is None:
            raise RuntimeError(f"no element for {selector_js}")
        return r

    def move(self, selector_js, dx=0, dy=0):
        x, y = self.center(selector_js)
        x, y = x + dx, y + dy
        self.js(f"Object.assign(document.getElementById('demo-cursor').style, {{left: '{x - 2}px', top: '{y - 2}px'}})")
        steps = 6
        for i in range(1, steps + 1):
            px = self.x + (x - self.x) * i / steps
            py = self.y + (y - self.y) * i / steps
            self.send("Input.dispatchMouseEvent", type="mouseMoved", x=px, y=py)
            self.hold(0.5 / steps)
        self.send("Input.dispatchMouseEvent", type="mouseMoved", x=x, y=y)
        self.x, self.y = x, y

    def click(self):
        self.js(f"""(() => {{
            const r = document.getElementById('demo-ring');
            Object.assign(r.style, {{ left: '{self.x}px', top: '{self.y}px', transition: 'none',
              transform: 'scale(.4)', opacity: 1 }});
            r.offsetWidth;
            Object.assign(r.style, {{ transition: 'transform .35s ease-out, opacity .35s ease-out',
              transform: 'scale(1.2)', opacity: 0 }});
        }})()""")
        for kind in ("mousePressed", "mouseReleased"):
            self.send("Input.dispatchMouseEvent", type=kind, x=self.x, y=self.y,
                      button="left", clickCount=1)

    def caption(self, text):
        self.js(f"""(() => {{
            const c = document.getElementById('demo-caption');
            c.textContent = {json.dumps(text)};
            c.style.opacity = {1 if text else 0};
        }})()""")


def q(selector):
    return f"document.querySelector({json.dumps(selector)})"


def cell(row, col):
    return f"[...document.querySelectorAll('#heat .cell')].find(b => b.getAttribute('aria-label').startsWith({json.dumps(f'{row}, {col}:')}))"


def legend_item(name):
    return f"[...document.querySelectorAll('#pastLegend button')].find(b => b.textContent === {json.dumps(name)})"


def segment(name):
    return f"[...document.querySelectorAll('#donut g.seg')].find(g => g.getAttribute('aria-label').startsWith({json.dumps(name + ',')}))"


def bake(out):
    data = json.loads((ROOT / "docs/demo-data.json").read_text())
    data["generated_at"] = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    html = (ROOT / "web/index.html").read_text()
    payload = json.dumps(data).replace("</", "<\\/")
    out.write_text(html.replace("/*__DATA__*/null", payload, 1))


def record(page):
    page.caption("")
    page.hold(1.0)

    page.caption("Allocation by asset class, straight from Sure")
    page.hold(1.4)

    page.caption("Hover a segment for its value")
    page.move(segment("Equities"))
    page.hold(1.3)

    page.caption("Click to drill in: class → account → holding")
    page.click()
    page.hold(1.2)
    page.move(q('.legend-row[data-name="Brokerage"] .legend-main'))
    page.click()
    page.hold(1.5)

    page.caption("Or group by market")
    page.move(q('[data-level="market"]'))
    page.click()
    page.hold(1.5)

    page.caption("The treemap keeps every holding the donut folds away")
    page.move(q('[data-level="item"]'))
    page.click()
    page.hold(0.9)
    page.move(q('[data-chart="treemap"]'))
    page.click()
    page.hold(2.2)
    page.move(q('[data-chart="donut"]'))
    page.click()
    page.move(q('[data-level="class"]'))
    page.click()
    page.hold(0.4)

    page.caption("Net worth nets the mortgage against the flat")
    page.move(q('[data-mode="net"]'))
    page.click()
    page.hold(1.8)

    page.caption("Exclude anything to see the rest")
    page.move(q('.legend-row[data-name="Property"] .legend-main'))
    page.hold(0.3)
    page.move(q('.legend-row[data-name="Property"] .legend-x'))
    page.click()
    page.hold(1.8)

    page.caption("Private mode hides amounts, keeps percentages")
    page.move(q("#privToggle"))
    page.click()
    page.hold(2.0)
    page.click()
    page.move(q("#exclReset"))
    page.click()
    page.move(q('[data-mode="assets"]'))
    page.click()
    page.hold(0.4)

    page.caption("Grid: market × asset class at a glance")
    page.move(q('[data-tab="grid"]'))
    page.click()
    page.hold(1.2)
    page.move(cell("United States", "Equities"))
    page.hold(1.6)

    page.caption("…or currency × asset class")
    page.move(q('[data-level="currency"]'))
    page.click()
    page.hold(1.8)

    page.caption("Over time, rebuilt from Sure's history")
    page.move(q('[data-tab="time"]'))
    page.click()
    page.hold(1.4)
    page.move("document.querySelectorAll('#pastChart .col')[4]")
    page.hold(1.6)

    page.caption("As shares of the whole")
    page.move(q('[data-scale="share"]'))
    page.click()
    page.hold(1.8)

    page.caption("The legend excludes here too")
    page.move(legend_item("Property"))
    page.click()
    page.move(q("#exclReset"))
    page.hold(1.8)
    page.click()
    page.caption("")
    page.hold(1.0)


def main():
    work = Path(tempfile.mkdtemp(prefix="sn-demo-"))
    html = work / "demo.html"
    bake(html)
    chrome = subprocess.Popen(
        ["chromium", "--headless=new", "--no-sandbox", "--disable-gpu", "--hide-scrollbars",
         "--force-device-scale-factor=1", f"--window-size={W},{H}", f"--remote-debugging-port={PORT}",
         f"--user-data-dir={work / 'profile'}", "--force-color-profile=srgb", "about:blank"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(100):
            try:
                targets = json.load(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json"))
                ws_url = next(t["webSocketDebuggerUrl"] for t in targets if t["type"] == "page")
                break
            except Exception:
                time.sleep(0.1)
        else:
            raise SystemExit("chromium did not start")

        page = Page(ws_url)
        page.send("Emulation.setDeviceMetricsOverride", width=W, height=H, deviceScaleFactor=1, mobile=False)
        page.send("Emulation.setEmulatedMedia", features=[{"name": "prefers-color-scheme", "value": "light"}])
        page.send("Page.enable")
        page.send("Page.navigate", url=html.as_uri())
        for _ in range(100):
            if page.js("!!document.querySelector('#donut g.seg')"):
                break
            time.sleep(0.1)
        page.js(OVERLAY)
        page.js(HISTORY)
        record(page)

        frames = work / "frames"
        frames.mkdir()
        import base64
        for i, data in enumerate(page.frames):
            (frames / f"{i:05d}.png").write_bytes(base64.b64decode(data))

        out = ROOT / "docs/demo.mp4"
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-framerate", str(FPS),
                        "-i", str(frames / "%05d.png"), "-c:v", "libx264", "-preset", "slow",
                        "-crf", "24", "-pix_fmt", "yuv420p", "-movflags", "+faststart",
                        str(out)], check=True)
        print(f"wrote {out} ({out.stat().st_size // 1024} KiB, {len(page.frames)} frames)")
    finally:
        chrome.terminate()
        chrome.wait()
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
