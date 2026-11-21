"""Opens the built page in headless Chromium and checks that it starts the library's
worker from the worker's own module, and that the worker answers the library's first
command.

usage: site.py SITE

SITE is the output of `nix build .#drawbar-web`.
"""

import functools
import http.server
import sys
import threading

from playwright.sync_api import sync_playwright

ANSWER_MS = 120_000

# What every worker the page starts posts back to it, in `self.heard`.
HEAR = """
self.heard = [];
self.Worker = class extends Worker {
  constructor(...args) {
    super(...args);
    this.addEventListener("message", ({ data }) => self.heard.push(data));
  }
};
"""

SETTLED = "self.heard.some((data) => 'failed' in data || ('id' in data && 'text' in data))"
FAILED = "self.heard.filter((data) => 'failed' in data).map((data) => data.failed)"


class Site(http.server.SimpleHTTPRequestHandler):
    extensions_map = {
        ".html": "text/html",
        ".js": "text/javascript",
        ".wasm": "application/wasm",
    }

    def log_message(self, *args):
        pass


def main(site):
    handler = functools.partial(Site, directory=site)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    url = f"http://127.0.0.1:{server.server_address[1]}/"

    with sync_playwright() as playwright:
        # eframe draws with WebGL2, which a headless browser has only in software.
        browser = playwright.chromium.launch(
            args=["--use-angle=swiftshader", "--enable-unsafe-swiftshader"]
        )
        page = browser.new_page()
        errors = []
        page.on("pageerror", lambda error: errors.append(str(error)))
        fetched = []
        page.on("requestfinished", lambda request: fetched.append(request.url))
        page.add_init_script(HEAR)
        page.goto(url)
        try:
            page.wait_for_function(SETTLED, timeout=ANSWER_MS)
        finally:
            failed = page.evaluate(FAILED)
            browser.close()

    assert not errors, f"the page threw: {errors}"
    assert not failed, f"a worker failed: {failed}"
    worker_wasm = [url for url in fetched if "/pkg/drawbar_worker_bg-" in url]
    assert worker_wasm, f"no worker fetched its module; fetched {fetched}"


if __name__ == "__main__":
    main(sys.argv[1])
