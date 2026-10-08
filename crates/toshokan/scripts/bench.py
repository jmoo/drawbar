#!/usr/bin/env python3
# Runs the browser benchmark (tests/bench.rs) in one browser with a profile kept between
# runs, so the generated library is written once, and reports the browser's memory with
# the library open.
#
# From crates/ in the development shell, with wasm-bindgen-cli, lld and the WebDriver on
# PATH and CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner:
#
#   toshokan/scripts/bench.py chrome --binary <chrome> --profile <dir> [--headed]
#   toshokan/scripts/bench.py firefox --binary <firefox> --profile <dir> [--headed]
#
# The origin private file system belongs to the origin, so every run of one profile uses
# the same --port.

import argparse
import json
import os
import signal
import subprocess
import sys
import time
import urllib.request

DRIVER_PORT = {"chrome": 9615, "firefox": 4544}


def request(method, url, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(req, timeout=600) as reply:
        return json.loads(reply.read() or b"null")


def wait_for(predicate, seconds, what):
    deadline = time.time() + seconds
    while time.time() < deadline:
        found = predicate()
        if found:
            return found
        time.sleep(1)
    sys.exit(f"timed out waiting for {what}")


def capabilities(args):
    if args.browser == "chrome":
        flags = [f"--user-data-dir={args.profile}", "--no-first-run", "--no-default-browser-check"]
        if not args.headed:
            flags.append("--headless=new")
        return {"browserName": "chrome", "goog:chromeOptions": {"binary": args.binary, "args": flags}}
    flags = ["-profile", args.profile] + ([] if args.headed else ["-headless"])
    return {"browserName": "firefox", "moz:firefoxOptions": {"binary": args.binary, "args": flags}}


def start_driver(args):
    port = DRIVER_PORT[args.browser]
    command = {
        "chrome": ["chromedriver", f"--port={port}"],
        "firefox": ["geckodriver", "--port", str(port)],
    }[args.browser]
    driver = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    base = f"http://127.0.0.1:{port}"

    def up():
        try:
            return request("GET", f"{base}/status")["value"]["ready"]
        except OSError:
            return False

    wait_for(up, 30, "the WebDriver")
    os.makedirs(args.profile, exist_ok=True)
    session = request(
        "POST", f"{base}/session", {"capabilities": {"alwaysMatch": capabilities(args)}}
    )["value"]["sessionId"]
    return driver, f"{base}/session/{session}"


def page_text(session):
    script = "return document.body ? document.body.innerText : ''"
    return request("POST", f"{session}/execute/sync", {"script": script, "args": []})["value"]


def run_phase(args, session, phase):
    env = dict(os.environ, NO_HEADLESS="1", WASM_BINDGEN_TEST_ADDRESS=f"127.0.0.1:{args.port}")
    if args.entities:
        env["TOSHOKAN_BENCH_ENTITIES"] = str(args.entities)
    command = [
        "cargo", "test", "--release", "-p", "toshokan", "--features", "web",
        "--target", "wasm32-unknown-unknown", "--test", "bench", "--", "--nocapture", phase,
    ]
    log = open(os.path.join(args.profile, f"runner-{phase}.log"), "w")
    runner = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT)
    try:
        def serving():
            with open(log.name) as text:
                return "available at" in text.read()

        wait_for(serving, 1800, "the test runner")
        request("POST", f"{session}/url", {"url": f"http://127.0.0.1:{args.port}/"})

        def finished():
            text = page_text(session)
            return text if "test result:" in text or "panicked" in text else None

        return wait_for(finished, args.timeout, f"the {phase} test")
    finally:
        for pid, _, _ in descendants(runner.pid):
            try:
                os.kill(pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        runner.terminate()
        runner.wait()


def descendants(root):
    table = subprocess.run(
        ["ps", "-axo", "pid=,ppid=,rss=,command="], capture_output=True, text=True
    ).stdout
    rows = {}
    for line in table.splitlines():
        pid, ppid, rss, command = line.split(None, 3)
        rows[int(pid)] = (int(ppid), int(rss), command)
    found, frontier = [], [root]
    while frontier:
        parent = frontier.pop()
        for pid, (ppid, rss, command) in rows.items():
            if ppid == parent:
                found.append((pid, rss, command))
                frontier.append(pid)
    return found


def footprint(pid):
    shown = subprocess.run(["footprint", str(pid)], capture_output=True, text=True).stdout
    for line in shown.splitlines():
        if "phys_footprint:" in line:
            return line.split("phys_footprint:")[1].strip()
    return "unknown"


def memory(driver):
    processes = descendants(driver.pid)
    content = [
        p for p in processes if "--type=renderer" in p[2] or "-isForBrowser" in p[2]
    ]
    lines = [f"browser processes: {len(processes)}, RSS {sum(p[1] for p in processes) / 1024:.0f} MB in all"]
    for pid, rss, _ in sorted(content, key=lambda p: -p[1])[:2]:
        lines.append(f"content process {pid}: RSS {rss / 1024:.0f} MB, footprint {footprint(pid)}")
    return lines


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("browser", choices=["chrome", "firefox"])
    parser.add_argument("--binary", required=True)
    parser.add_argument("--profile", required=True)
    parser.add_argument("--headed", action="store_true")
    parser.add_argument("--port", type=int, default=8792)
    parser.add_argument("--entities", type=int)
    parser.add_argument("--timeout", type=int, default=3600)
    args = parser.parse_args()
    args.profile = os.path.abspath(args.profile)

    driver, session = start_driver(args)
    try:
        for phase in ["generate", "measure"]:
            text = run_phase(args, session, phase)
            print(f"== {phase}\n{text}")
            if "panicked" in text or "test result: ok" not in text:
                sys.exit(f"{phase} failed")
        visible = request(
            "POST", f"{session}/execute/sync",
            {"script": "return document.visibilityState", "args": []},
        )["value"]
        print(f"== memory (page {visible})")
        print("\n".join(memory(driver)))
    finally:
        try:
            request("DELETE", session)
        finally:
            driver.terminate()


if __name__ == "__main__":
    main()
