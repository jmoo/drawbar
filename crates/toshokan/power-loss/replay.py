"""Replays a toshokan workload's logged writes to every crash point and checks each.

usage: replay FS MKFS WORKLOAD SAMPLES SEED

MKFS is the command that makes FS on the device appended to it.

The workload runs on FS over a dm-log-writes target, which logs every write,
flush and FUA, and the marks power_loss drops once each step returns. The log is
then replayed onto a fresh device up to each flush after the setup mark. Between
two flushes, SAMPLES further states each keep every FUA write before a random
cut and each other write before it with even odds, as the device may persist any
of them. Each state is mounted on a throwaway snapshot, so the file system's own
recovery runs, and checked by power_loss with the steps marked before it as
acknowledged.
"""

import collections
import random
import re
import subprocess
import sys
import time

DATA = "/dev/disk/by-id/virtio-data"
LOG = "/dev/disk/by-id/virtio-log"
REPLAY = "/dev/disk/by-id/virtio-replay"
COW = "/dev/disk/by-id/virtio-cow"
LOGGED = "power-loss-log"
SNAPSHOT = "power-loss-snapshot"
RUN = "/mnt/run"
CHECK = "/mnt/check"

FLUSH = 1 << 0
FUA = 1 << 1
MARK = 1 << 3


def sh(*args, check=True):
    done = subprocess.run(args, capture_output=True, text=True)
    if check and done.returncode != 0:
        raise RuntimeError(f"{' '.join(args)}: {done.stdout}{done.stderr}")
    return done


def sectors(device):
    return sh("blockdev", "--getsz", device).stdout.strip()


def replay(target, start, count):
    sh("replay-log", "--log", LOG, "--replay", target,
       "--start-entry", str(start), "--limit", str(count))


def remove(device):
    """Removes a device-mapper device once udev's probes of it let it go."""
    sh("udevadm", "settle")
    sh("dmsetup", "remove", "--retry", device)


def find_mark(label):
    found = sh("replay-log", "--log", LOG, "--find", "--end-mark", label)
    return int(found.stdout.split("@")[0])


def entries():
    """The flags of every logged entry, in order."""
    found = sh("replay-log", "--log", LOG, "--find", "--end-mark", "end", "-vv")
    pattern = re.compile(r"seek entry (\d+)@\d+: \d+, size \d+, flags (0x[0-9a-f]+)")
    flags = [int(m.group(2), 16) for m in pattern.finditer(found.stdout)]
    if not flags:
        raise RuntimeError(f"no entries in {found.stdout[:500]}")
    return flags


def record(mkfs, workload):
    sh("blkdiscard", "-f", "-z", DATA)
    sh("blkdiscard", "-f", "-z", REPLAY)
    sh("dmsetup", "create", LOGGED, "--table",
       f"0 {sectors(DATA)} log-writes {DATA} {LOG}")
    logged = f"/dev/mapper/{LOGGED}"
    sh(*mkfs.split(), logged)
    sh("mkdir", "-p", RUN)
    sh("mount", "-o", "noatime", logged, RUN)
    sh("power_loss", "run", workload, f"{RUN}/{workload}", f"dm:{LOGGED}")
    sh("dmsetup", "message", LOGGED, "0", "mark", "end")
    sh("umount", RUN)
    remove(LOGGED)


def check(fs, workload, acked, extra):
    """Checks the replay device's state, with the entries `extra` written over
    it, on a snapshot: what the check found, or why it failed."""
    sh("dmsetup", "create", SNAPSHOT, "--table",
       f"0 {sectors(REPLAY)} snapshot {REPLAY} {COW} N 8")
    snapshot = f"/dev/mapper/{SNAPSHOT}"
    try:
        for i in extra:
            replay(snapshot, i, 1)
        if fs == "btrfs":
            sh("btrfs", "device", "scan", "--forget")
        mounted = sh("mount", snapshot, CHECK, check=False)
        if mounted.returncode != 0:
            return f"mount failed: {mounted.stderr.strip()}"
        try:
            checked = sh("power_loss", "check", workload,
                         f"{CHECK}/{workload}", str(acked), check=False)
        finally:
            sh("umount", CHECK)
        if checked.returncode != 0:
            return checked.stderr.strip()
        return dict(f.split("=") for f in checked.stdout.split()[2:])
    finally:
        remove(SNAPSHOT)


def main():
    fs, mkfs, workload, samples, seed = sys.argv[1:]
    samples = int(samples)
    rng = random.Random(f"{seed} {fs} {workload}")
    steps = dict(
        line.split() for line in sh("power_loss", "list").stdout.splitlines()
    )[workload]
    started = time.monotonic()
    record(mkfs, workload)
    flags = entries()
    marks = [find_mark(f"step-{i}") for i in range(1, int(steps) + 1)]
    setup = find_mark("setup")
    sh("mkdir", "-p", CHECK)

    def acked(before):
        return sum(1 for mark in marks if mark < before)

    failures = []
    found = collections.Counter()

    def checked(at, what, extra=()):
        found["crash states"] += 1
        result = check(fs, workload, acked(at), extra)
        if isinstance(result, str):
            failures.append(f"{fs} {workload} {what}, {acked(at)} acked: {result}")
            print(failures[-1], flush=True)
            return
        found[f"start {result['start']}"] += 1
        found["in flight"] += result["model"] != result["acked"]
        found["settled"] += result["settled"] != "0"
        found["orphaned"] += result["orphans"] != "0"
        found["with an unreadable log"] += result["unreadable"] != "0"

    replay(REPLAY, 0, setup + 1)
    checked(setup + 1, f"at the setup mark, entry {setup}")
    flushes = [i for i, f in enumerate(flags) if f & FLUSH and i > setup]
    cur = setup + 1
    for point in flushes + [len(flags) - 1]:
        if point < cur:
            continue
        writes = [i for i in range(cur, point) if not flags[i] & MARK]
        for _ in range(samples if len(writes) > 1 else 0):
            cut = rng.randint(1, len(writes))
            kept = [i for i in writes[:cut]
                    if flags[i] & FUA or rng.random() < 0.5]
            checked(writes[cut - 1] + 1,
                    f"entries {kept} of {writes[0]}..{writes[cut - 1]}", kept)
        replay(REPLAY, cur, point - cur + 1)
        cur = point + 1
        checked(point + 1, f"at entry {point}")
    took = time.monotonic() - started
    tally = ", ".join(f"{n} {what}" for what, n in sorted(found.items()))
    print(f"{fs} {workload}: {len(flags)} entries, {len(flushes)} flushes, "
          f"{len(failures)} failed, {took:.0f} s; {tally}", flush=True)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
