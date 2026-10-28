"""Replays toshokan workloads' logged writes to every crash point and checks each.

usage: replay FS MKFS SAMPLES SEED WORKLOAD...

MKFS is the command that makes FS on the device appended to it.

Each workload runs at once with the others, in its own slice of the four disks.
It runs on FS over a dm-log-writes target, which logs every write, flush and
FUA, and the marks power_loss drops once each step returns. The log is then
replayed onto a fresh device up to each flush after the setup mark. Between two
flushes, the device may persist any of the writes: one state each keeps all of
them but one, and SAMPLES further states each keep every FUA write before a
random cut and each other write before it with even odds. A write itself is
replayed whole. Each state is mounted on a throwaway snapshot, so the file
system's own recovery runs, and checked by power_loss with the steps marked
before it as acknowledged, and the entries their marks name. A state that keeps
the same writes over the same flush, with the same steps acknowledged, as one
already checked is the same disk under the same check, and counts that check's
result. An exFAT state that fsck.exfat finds corrupt is checked too, but counted
apart and never fails the run: no protocol above the file system can keep it
whole.
"""

import collections
import concurrent.futures
import random
import re
import subprocess
import sys
import time

DISKS = ("data", "log", "replay", "cow")

FLUSH = 1 << 0
FUA = 1 << 1
MARK = 1 << 3


def sh(*args, check=True):
    done = subprocess.run(args, capture_output=True, text=True)
    if check and done.returncode != 0:
        raise RuntimeError(f"{' '.join(args)}: {done.stdout}{done.stderr}")
    return done


def sectors(device):
    return int(sh("blockdev", "--getsz", device).stdout.strip())


def remove(device):
    """Removes a device-mapper device once udev's probes of it let it go."""
    sh("udevadm", "settle")
    sh("dmsetup", "remove", "--retry", device)


def slices(count):
    """Splits each disk into `count` linear devices; the devices of each slice."""
    found = [{} for _ in range(count)]
    for disk in DISKS:
        whole = f"/dev/disk/by-id/virtio-{disk}"
        size = sectors(whole) // count
        for i, devices in enumerate(found):
            name = f"power-loss-{disk}-{i}"
            sh("dmsetup", "create", name, "--table", f"0 {size} linear {whole} {i * size}")
            devices[disk] = f"/dev/mapper/{name}"
    sh("udevadm", "settle")
    return found


class Replay:
    """One workload's record, replay and checks, on one slice of the disks."""

    def __init__(self, fs, mkfs, workload, samples, seed, slot, devices):
        self.fs, self.mkfs, self.workload, self.samples = fs, mkfs, workload, samples
        self.rng = random.Random(f"{seed} {fs} {workload}")
        self.data, self.log = devices["data"], devices["log"]
        self.base, self.cow = devices["replay"], devices["cow"]
        self.logged = f"power-loss-logged-{slot}"
        self.snapshot = f"power-loss-snapshot-{slot}"
        self.run_dir = f"/mnt/run-{slot}"
        self.check_dir = f"/mnt/check-{slot}"
        self.summary = None

    def replay(self, target, start, count):
        sh("replay-log", "--log", self.log, "--replay", target,
           "--start-entry", str(start), "--limit", str(count))

    def find_mark(self, label):
        found = sh("replay-log", "--log", self.log, "--find", "--end-mark", label)
        return int(found.stdout.split("@")[0])

    def entries(self):
        """The flags of every logged entry, in order."""
        found = sh("replay-log", "--log", self.log, "--find", "--end-mark", "end", "-vv")
        pattern = re.compile(r"seek entry (\d+)@\d+: \d+, size \d+, flags (0x[0-9a-f]+)")
        flags = [int(m.group(2), 16) for m in pattern.finditer(found.stdout)]
        if not flags:
            raise RuntimeError(f"no entries in {found.stdout[:500]}")
        return flags

    def record(self):
        """Runs the workload over the logging target; the labels of its step marks."""
        sh("blkdiscard", "-f", "-z", self.data)
        sh("blkdiscard", "-f", "-z", self.base)
        sh("dmsetup", "create", self.logged, "--table",
           f"0 {sectors(self.data)} log-writes {self.data} {self.log}")
        logged = f"/dev/mapper/{self.logged}"
        sh(*self.mkfs.split(), logged)
        sh("mkdir", "-p", self.run_dir)
        sh("mount", "-o", "noatime", logged, self.run_dir)
        ran = sh("power_loss", "run", self.workload, f"{self.run_dir}/{self.workload}",
                 f"dm:{self.logged}")
        sh("dmsetup", "message", self.logged, "0", "mark", "end")
        sh("umount", self.run_dir)
        remove(self.logged)
        return ran.stdout.split()

    def check(self, labels, extra):
        """Checks the replay device's state, with the runs of log entries `extra`
        written over it, on a snapshot, once the steps `labels` mark returned:
        whether fsck.exfat finds it corrupt, and what the check found or why it
        failed."""
        sh("dmsetup", "create", self.snapshot, "--table",
           f"0 {sectors(self.base)} snapshot {self.base} {self.cow} N 8")
        snapshot = f"/dev/mapper/{self.snapshot}"
        try:
            for start, count in extra:
                self.replay(snapshot, start, count)
            if self.fs == "btrfs":
                sh("btrfs", "device", "scan", "--forget")
            corrupt = self.fs == "exfat" and sh("fsck.exfat", "-n", snapshot, check=False).returncode != 0
            mounted = sh("mount", snapshot, self.check_dir, check=False)
            if mounted.returncode != 0:
                return corrupt, f"mount failed: {mounted.stderr.strip()}"
            entries = [label.split(":")[1] for label in labels if ":" in label]
            try:
                checked = sh("power_loss", "check", self.workload, f"{self.check_dir}/{self.workload}",
                             str(len(labels)), *entries, check=False)
            finally:
                sh("umount", self.check_dir)
            if checked.returncode != 0:
                return corrupt, checked.stderr.strip()
            return corrupt, dict(f.split("=") for f in checked.stdout.split()[2:])
        finally:
            remove(self.snapshot)

    def __call__(self):
        fs, workload, rng = self.fs, self.workload, self.rng
        started = time.monotonic()
        labels = self.record()
        flags = self.entries()
        marks = [(self.find_mark(label), label) for label in labels]
        setup = self.find_mark("setup")
        sh("mkdir", "-p", self.check_dir)

        def acked(before):
            return [label for mark, label in marks if mark < before]

        failures = []
        found = collections.Counter()
        results = {}
        cur = setup + 1

        def checked(at, what, extra=()):
            found["crash states"] += 1
            labels = acked(at)
            key = (cur, tuple(extra), tuple(labels))
            if key in results:
                found["repeated"] += 1
            else:
                results[key] = self.check(labels, runs(extra, flags))
            corrupt, result = results[key]
            found["flagged by fsck.exfat"] += corrupt
            if isinstance(result, str):
                shown = f"{fs} {workload} {what}, {len(labels)} acked: {result}"
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

        self.replay(self.base, 0, cur)
        checked(cur, f"at the setup mark, entry {setup}")
        flushes = [i for i, f in enumerate(flags) if f & FLUSH and i > setup]
        for point in flushes + [len(flags) - 1]:
            if point < cur:
                continue
            writes = [i for i in range(cur, point) if not flags[i] & MARK]
            for lost in writes if len(writes) > 1 else []:
                if flags[lost] & FUA:
                    continue
                kept = [i for i in writes if i != lost]
                checked(writes[-1] + 1, f"entries {writes[0]}..{writes[-1]} but {lost}", kept)
            for _ in range(self.samples if len(writes) > 1 else 0):
                cut = rng.randint(1, len(writes))
                kept = [i for i in writes[:cut]
                        if flags[i] & FUA or rng.random() < 0.5]
                checked(writes[cut - 1] + 1,
                        f"entries {kept} of {writes[0]}..{writes[cut - 1]}", kept)
            self.replay(self.base, cur, point - cur + 1)
            cur = point + 1
            checked(cur, f"at entry {point}")
        took = time.monotonic() - started
        tally = ", ".join(f"{n} {what}" for what, n in sorted(found.items()))
        self.summary = (f"{fs} {workload}: {len(flags)} entries, {len(flushes)} flushes, "
                        f"{len(failures)} failed, {took:.0f} s; {tally}")
        print(self.summary, flush=True)
        return not failures


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


def main():
    fs, mkfs, samples, seed, *workloads = sys.argv[1:]
    replays = [Replay(fs, mkfs, workload, int(samples), seed, slot, devices)
               for slot, (workload, devices) in enumerate(zip(workloads, slices(len(workloads))))]
    with concurrent.futures.ThreadPoolExecutor(len(replays)) as pool:
        passed = list(pool.map(lambda replay: replay(), replays))
    print("\n".join(replay.summary for replay in replays), flush=True)
    failed = [replay.workload for replay, ok in zip(replays, passed) if not ok]
    if failed:
        print(f"crash states of {failed} failed their checks", flush=True)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
