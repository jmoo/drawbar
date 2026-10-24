"""Replays a toshokan workload's logged writes to every crash point and checks each.

usage: replay FS MKFS WORKLOAD SAMPLES SEED

MKFS is the command that makes FS on the device appended to it.

The workload runs on FS over a dm-log-writes target, which logs every write,
flush and FUA, and the marks power_loss drops once each step returns. The log is
then replayed onto a fresh device up to each flush after the setup mark. Between
two flushes, the device may persist any of the writes: one state each keeps all
of them but one, and SAMPLES further states each keep every FUA write before a
random cut and each other write before it with even odds. A write itself is
replayed whole. Each state is mounted on a throwaway snapshot, so the file
system's own recovery runs, and checked by power_loss with the steps marked
before it as acknowledged, and the entries their marks name. An exFAT state that
fsck.exfat finds corrupt is checked too, but counted apart and never fails the
run: no protocol above the file system can keep it whole.
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
    """Runs the workload over the logging target; the labels of its step marks."""
    sh("blkdiscard", "-f", "-z", DATA)
    sh("blkdiscard", "-f", "-z", REPLAY)
    sh("dmsetup", "create", LOGGED, "--table",
       f"0 {sectors(DATA)} log-writes {DATA} {LOG}")
    logged = f"/dev/mapper/{LOGGED}"
    sh(*mkfs.split(), logged)
    sh("mkdir", "-p", RUN)
    sh("mount", "-o", "noatime", logged, RUN)
    ran = sh("power_loss", "run", workload, f"{RUN}/{workload}", f"dm:{LOGGED}")
    sh("dmsetup", "message", LOGGED, "0", "mark", "end")
    sh("umount", RUN)
    remove(LOGGED)
    return ran.stdout.split()


def runs(kept, flags):
    """`kept`, ascending log entries, as runs (first, count) that also take in
    the marks between them, which write nothing."""
    found = []
    for i in kept:
        if found and all(flags[j] & MARK for j in range(sum(found[-1]), i)):
            found[-1] = (found[-1][0], i - found[-1][0] + 1)
        else:
            found.append((i, 1))
    return found


def check(fs, workload, labels, extra):
    """Checks the replay device's state, with the runs of log entries `extra`
    written over it, on a snapshot, once the steps `labels` mark returned: whether
    fsck.exfat finds it corrupt, and what the check found or why it failed."""
    sh("dmsetup", "create", SNAPSHOT, "--table",
       f"0 {sectors(REPLAY)} snapshot {REPLAY} {COW} N 8")
    snapshot = f"/dev/mapper/{SNAPSHOT}"
    try:
        for start, count in extra:
            replay(snapshot, start, count)
        if fs == "btrfs":
            sh("btrfs", "device", "scan", "--forget")
        corrupt = fs == "exfat" and sh("fsck.exfat", "-n", snapshot, check=False).returncode != 0
        mounted = sh("mount", snapshot, CHECK, check=False)
        if mounted.returncode != 0:
            return corrupt, f"mount failed: {mounted.stderr.strip()}"
        entries = [label.split(":")[1] for label in labels if ":" in label]
        try:
            checked = sh("power_loss", "check", workload, f"{CHECK}/{workload}",
                         str(len(labels)), *entries, check=False)
        finally:
            sh("umount", CHECK)
        if checked.returncode != 0:
            return corrupt, checked.stderr.strip()
        return corrupt, dict(f.split("=") for f in checked.stdout.split()[2:])
    finally:
        remove(SNAPSHOT)


def main():
    fs, mkfs, workload, samples, seed = sys.argv[1:]
    samples = int(samples)
    rng = random.Random(f"{seed} {fs} {workload}")
    started = time.monotonic()
    labels = record(mkfs, workload)
    flags = entries()
    marks = [(find_mark(label), label) for label in labels]
    setup = find_mark("setup")
    sh("mkdir", "-p", CHECK)

    def acked(before):
        return [label for mark, label in marks if mark < before]

    failures = []
    found = collections.Counter()

    def checked(at, what, extra=()):
        found["crash states"] += 1
        corrupt, result = check(fs, workload, acked(at), runs(extra, flags))
        found["flagged by fsck.exfat"] += corrupt
        if isinstance(result, str):
            shown = f"{fs} {workload} {what}, {len(acked(at))} acked: {result}"
            if corrupt:
                found["flagged and failed"] += 1
                print(f"out of scope, flagged by fsck.exfat: {shown}", flush=True)
                return
            failures.append(shown)
            print(shown, flush=True)
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
        for lost in writes if len(writes) > 1 else []:
            if flags[lost] & FUA:
                continue
            kept = [i for i in writes if i != lost]
            checked(writes[-1] + 1, f"entries {writes[0]}..{writes[-1]} but {lost}", kept)
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
