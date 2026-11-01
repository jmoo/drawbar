"""Kills toshokan's power-loss workloads partway through, in real browsers, and checks
what each kill left.

usage: toshokan-browser-crash SITE [--browsers B,…] [--stride N] [--workloads W,…]
       toshokan-browser-crash SITE --quota N [--browsers B,…]

SITE holds the crash page: index.html, its scripts and the crash module. Each workload
runs once whole in a fresh directory of the origin private file system, which counts
the mutating requests its steps make. Each case then runs it again in a fresh
directory and stops it at one of those requests, before sending it or while the
browser performs it, and kills it there as `KILLS` lists. A page of the browser,
started again where the kill took it, checks the directory with the power-loss
checker: each library path holds its old bytes, its new bytes, or nothing while a
pending record names it; every entry an acknowledged step committed is there;
recovery reaches the model before or after the step in flight; settling again
changes nothing; the next write succeeds.

--stride N takes every Nth request of each workload, starting at a different one for
each workload; 1 takes them all. The kills, and stopping before or during a request,
take turns along the requests taken. --quota N runs the quota case instead, with the
first N of `QUOTA_ROOMS`, in the browsers `QUOTA` can shrink an origin's quota in.
"""

import argparse
import functools
import http.server
import itertools
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time

from playwright.sync_api import Error as PlaywrightError
from playwright.sync_api import sync_playwright

PARENT = "crash"
# A step's mark names the entry it committed after a colon.
STEP = "step-"
MESSAGE_MS = 120_000
LOCK_MS = 10_000


class Page(http.server.SimpleHTTPRequestHandler):
    """Serves SITE cross-origin isolated, which shared memory needs."""

    extensions_map = {
        ".html": "text/html",
        ".js": "text/javascript",
        ".wasm": "application/wasm",
    }

    def end_headers(self):
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def log_message(self, *args):
        pass


def serve(site):
    handler = functools.partial(Page, directory=site)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return f"http://127.0.0.1:{server.server_address[1]}/"


class Browser:
    """One browser on a profile of its own, which a kill leaves for the next launch,
    steering the crash page."""

    def __init__(self, playwright, name, url, profile):
        self.kind = getattr(playwright, name)
        self.name, self.url, self.profile = name, url, profile
        self.install = os.path.dirname(os.path.realpath(self.kind.executable_path))
        self.launch()

    def launch(self):
        prefs = PREFS if self.name == "firefox" else None
        self.context = self.kind.launch_persistent_context(
            self.profile, args=ARGS.get(self.name, []), firefox_user_prefs=prefs
        )
        self.page = self.context.pages[0]
        said = []
        self.page.on("console", lambda message: said.append(message.text))
        self.page.on("pageerror", lambda error: said.append(str(error)))
        self.page.goto(self.url)
        try:
            self.page.wait_for_function("document.title === 'ready'")
        except PlaywrightError as error:
            raise RuntimeError(f"the crash page did not start: {said}") from error

    def start(self, entry, fields):
        self.page.evaluate("([entry, fields]) => harness.start(entry, fields)", [entry, fields])

    def next(self):
        return self.page.evaluate("ms => harness.next(ms)", MESSAGE_MS)

    def kill(self, how):
        """Kills the page's workers as `how` says, and starts the browser again where
        that took the page with them."""
        if how == "terminate":
            self.page.evaluate("harness.terminate()")
            return
        if how == "crash":
            sigkill([pid for pid, line in started(self.install) if CONTENT[self.name] in line])
        elif how == "kill":
            sigkill([pid for pid, _ in started(self.install)])
        try:
            self.context.close()
        except Exception:
            pass
        self.launch()

    def check(self, workload, run, ran):
        """Checks what `ran`, a run of `workload` in `run`, left there."""
        waited = self.page.evaluate("([run, ms]) => harness.locksFree(run, ms)", [run, LOCK_MS])
        if waited < 0:
            return {"failed": f"the killed run's locks were still held after {LOCK_MS} ms"}
        marks = steps(ran)
        fields = {
            "workload": workload,
            "run": f"{PARENT}/{run}",
            "lost": ran["lost"],
            "acked": len(marks),
            "entries": [mark.split(":", 1)[1] for mark in marks if ":" in mark],
        }
        self.start("crashCheck", fields)
        checked = self.next()
        self.page.evaluate("([parent, name]) => harness.wipe([parent], name)", [PARENT, run])
        return checked

    def close(self):
        self.context.close()


def steps(ran):
    """The marks of the steps that returned."""
    return [mark for mark in ran["marks"] if mark.startswith(STEP)]


# WebKit runs toshokan only on macOS: WebKitGTK and WPE refuse a sync access handle
# with NotSupportedError. Measured in Playwright's WPE build.
BROWSERS = ["chromium", "firefox"] + (["webkit"] if sys.platform == "darwin" else [])

# Chromium's own sandbox cannot start inside the Nix build sandbox.
ARGS = {"chromium": ["--no-sandbox", "--disable-dev-shm-usage"]}
# A 50 MiB limit for all of Firefox's storage, which gives an origin 10 MiB.
PREFS = {"dom.quotaManager.temporaryStorage.fixedLimit": 50 << 10}

# What a command line holds in each process that runs a page and its workers.
CONTENT = {
    "chromium": "--type=renderer",
    "firefox": "-isForBrowser",
    "webkit": "WebContent",
}

# How a run is killed: its page's workers terminated, the browser quit, the process
# running the page killed, or every process of the browser killed.
KILLS = ["terminate", "close", "crash", "kill"]


def processes():
    """Every process, as (pid, parent, state, command line)."""
    listed = ["ps", "-A", "-o", "pid=,ppid=,stat=,command="]
    for line in subprocess.run(listed, capture_output=True, text=True, check=True).stdout.splitlines():
        pid, parent, state, command = line.split(None, 3)
        yield int(pid), int(parent), state, command


def started(install):
    """The browser's processes, with their command lines: those Playwright's driver,
    this one's child, started, directly or not, and those of `install` that launchd
    started for it, as macOS starts WebKit's."""
    listed = list(processes())
    ours = {pid for pid, parent, _, _ in listed if parent == os.getpid()}
    found = {}
    while True:
        more = {
            pid: command
            for pid, parent, _, command in listed
            if pid not in found
            and (parent in ours | found.keys() or parent == 1 and command.startswith(install))
        }
        if not more:
            return found.items()
        found |= more


def sigkill(pids):
    """Kills `pids` with SIGKILL and waits until each is gone."""
    if not pids:
        raise RuntimeError("no process to kill")
    for pid in pids:
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    while any(pid in pids and "Z" not in state for pid, _, state, _ in processes()):
        time.sleep(0.01)


def sweep(browser, stride, only):
    """Runs every case the stride takes in `browser`, and returns the failures."""
    failures, tally = [], {}
    names = browser.page.evaluate("harness.workloads()")
    for index, workload in enumerate(names):
        if only and workload not in only:
            continue
        began, before = time.time(), len(failures)
        run = f"{browser.name}-{workload}-whole"
        browser.start("crashRun", {"workload": workload, "run": f"{PARENT}/{run}"})
        whole = browser.next()
        if "done" not in whole:
            failures.append(f"{workload} whole: {whole.get('failed', whole)}")
            continue
        checked = browser.check(workload, run, whole)
        if "checked" not in checked:
            failures.append(f"{workload} whole, then checked: {checked.get('failed')}")
        taken = range(index % stride, whole["done"], stride)
        turns = itertools.cycle(itertools.product([False, True], KILLS))
        for at, (sent, how) in zip(taken, turns):
            case = f"{workload} at request {at} {'sent' if sent else 'unsent'}, {how}"
            run = f"{browser.name}-{workload}-{at}-{int(sent)}-{how}"
            fields = {"workload": workload, "run": f"{PARENT}/{run}", "stop": at, "sent": sent}
            browser.start("crashRun", fields)
            ran = browser.next()
            if "parked" not in ran:
                failures.append(f"{case}: the run did not stop: {ran.get('failed', ran)}")
                continue
            browser.kill(how)
            checked = browser.check(workload, run, ran)
            if "checked" not in checked:
                failures.append(f"{case}: {checked.get('failed', checked)}")
                continue
            past = checked["checked"]["model"] > len(steps(ran))
            tally[how] = tally.get(how, 0) + 1
            tally["past the step in flight"] = tally.get("past the step in flight", 0) + past
        cases = len(range(index % stride, whole["done"], stride))
        print(
            f"{browser.name} {workload}: {cases} of {whole['done']} requests,"
            f" {len(failures) - before} failed, {time.time() - began:.0f} s",
            flush=True,
        )
    return failures, tally


# How a write the quota refused may fail.
REFUSED = re.compile(r"(read-only: )?no space at ")
QUOTA_FILE = 128 << 10
# The room above the origin's usage each quota run is given: enough for several
# imports, stepped so the refusal lands on different requests.
# Chromium refuses any write with less than 1 MiB of room: a sync access handle
# reserves that much at least. Measured.
QUOTA_ROOMS = range((1 << 20) + (16 << 10), 2 << 20, 37 << 10)


def quota(browser, rooms):
    """Runs the quota case once with each room in `rooms`."""
    failures, tally = [], {}
    for room in rooms:
        failed, shown = quota_once(browser, room)
        failures += [f"quota with {room} bytes of room: {failure}" for failure in failed]
        key = tuple(re.sub("[0-9a-f]{32}", "*", shown.get(at, "")) for at in ("refused", "again"))
        tally[key] = tally.get(key, 0) + 1
    return failures, tally


def quota_once(browser, room):
    """Imports until the origin has `room` bytes of its quota left, tries the refused
    import again, makes room, imports it once more, and checks after a kill."""
    grow = QUOTA[browser.name](browser, room)
    run = f"{browser.name}-quota-{room}"
    browser.page.evaluate(
        "fields => harness.quota(fields)",
        {"run": f"{PARENT}/{run}", "size": QUOTA_FILE},
    )
    full = browser.next()
    grow()
    if "full" not in full:
        return [full.get("failed", full)], {}
    failures = []
    refused, again = full["full"]["refused"], full["full"]["again"]
    if not REFUSED.match(refused):
        failures.append(f"the refused import failed with {refused}")
    if not REFUSED.match(again):
        failures.append(f"the same import before room was made: {again}")
    browser.page.evaluate("harness.roomMade()")
    after = browser.next()
    if after.get("after") != "ok":
        failures.append(f"the same import once room was made: {after.get('after', after)}")
    browser.kill("close")
    checked = browser.check(f"quota-{QUOTA_FILE}", run, after)
    if "checked" not in checked:
        failures.append(checked.get("failed", checked))
    return failures, {"refused": refused, "again": again}


def chromium_quota(browser, room):
    """Shrinks the origin's quota through the DevTools protocol."""
    session = browser.context.new_cdp_session(browser.page)
    origin = browser.url.rstrip("/")
    usage = browser.page.evaluate("harness.estimate()")["usage"]
    session.send("Storage.overrideQuotaForOrigin", {"origin": origin, "quotaSize": usage + room})
    return lambda: session.send("Storage.overrideQuotaForOrigin", {"origin": origin})


def firefox_quota(browser, room):
    """Fills the quota `PREFS` gives Firefox's origin with ballast, and empties it."""
    estimate = browser.page.evaluate("harness.estimate()")
    browser.page.evaluate("bytes => harness.ballast(bytes)", estimate["quota"] - estimate["usage"] - room)
    return lambda: browser.page.evaluate("harness.ballast(0)")


QUOTA = {"chromium": chromium_quota, "firefox": firefox_quota}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("site")
    parser.add_argument("--browsers", default=",".join(BROWSERS))
    parser.add_argument("--stride", type=int, default=1)
    parser.add_argument("--workloads", default="")
    parser.add_argument("--quota", type=int, default=0, metavar="N")
    args = parser.parse_args()
    url = serve(args.site)
    only = set(filter(None, args.workloads.split(",")))
    failed = 0
    with sync_playwright() as playwright:
        for name in args.browsers.split(","):
            if args.quota and name not in QUOTA:
                print(f"{name}: no quota case", flush=True)
                continue
            began = time.time()
            profile = tempfile.mkdtemp(prefix=f"crash-{name}-")
            browser = Browser(playwright, name, url, profile)
            if args.quota:
                failures, tally = quota(browser, QUOTA_ROOMS[: args.quota])
            else:
                failures, tally = sweep(browser, args.stride, only)
            browser.close()
            shutil.rmtree(profile, ignore_errors=True)
            print(f"{name}: {len(failures)} failed, {tally}, {time.time() - began:.0f} s", flush=True)
            for failure in failures:
                print(f"  {failure}", flush=True)
            failed += len(failures)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
