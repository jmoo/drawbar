# toshokan's native backend under power loss on real Linux file systems, in
# NixOS VMs. `smoke` crashes a VM mid-workload; `replay` replays each workload's
# logged writes to every flush and to sampled states between flushes.
{
  coreutils,
  gnugrep,
  gnused,
  lib,
  lvm2,
  power-loss,
  runCommand,
  testers,
  writeShellApplication,
  writers,
  xfstests,
}:
let
  filesystems = [
    "btrfs"
    "exfat"
    "ext4"
    "xfs"
  ];

  # No discards, and no initialization left to run in the background of a log.
  mkfs = {
    btrfs = "mkfs.btrfs -q -f --nodiscard";
    exfat = "mkfs.exfat";
    ext4 = "mkfs.ext4 -q -F -E lazy_itable_init=0,lazy_journal_init=0,nodiscard";
    xfs = "mkfs.xfs -q -f -K";
  };

  # The states a replay samples between two flushes.
  samples = 16;

  # The workloads a replay runs at once, each in its own slice of the disks, and
  # the machine that runs them.
  workloads = 8;
  replayMachine = {
    cores = 4;
    memorySize = 4096;
  };

  replay-log = runCommand "replay-log" { } ''
    mkdir -p "$out/bin"
    ln -s ${xfstests}/lib/xfstests/src/log-writes/replay-log "$out/bin/replay-log"
  '';

  replay = writers.writePython3Bin "power-loss-replay" { flakeIgnore = [ "E501" ]; } ./replay.py;

  # Runs every workload in turn under /mnt/FS, round after round, until the VM
  # is crashed; each run marks its steps in a synced file on the root disk.
  smoke-run = writeShellApplication {
    name = "power-loss-smoke-run";
    runtimeInputs = [
      coreutils
      power-loss
    ];
    text = ''
      fs=$1 round=$2
      mkdir -p "/var/lib/power-loss/$fs"
      for ((i = 0; ; i++)); do
        for workload in $(power_loss list | cut -d' ' -f1); do
          name="r$round-$i-$workload"
          power_loss run "$workload" "/mnt/$fs/$name" "file:/var/lib/power-loss/$fs/$name"
        done
      done
    '';
  };

  # Checks every run of round ROUND on FS whose setup was marked, with the steps
  # it marked, and the entries they name, as acknowledged.
  smoke-check = writeShellApplication {
    name = "power-loss-smoke-check";
    runtimeInputs = [
      coreutils
      gnugrep
      gnused
      power-loss
    ];
    text = ''
      fs=$1 round=$2 failed=0 checked=0
      for marks in "/var/lib/power-loss/$fs/r$round-"*; do
        grep -qx setup "$marks" || continue
        name=$(basename "$marks")
        workload=''${name#r*-*-}
        acked=$(grep -c '^step-' "$marks" || true)
        mapfile -t entries < <(sed -n 's/^step-[0-9]*://p' "$marks")
        checked=$((checked + 1))
        power_loss check "$workload" "/mnt/$fs/$name" "$acked" "''${entries[@]}" || failed=$((failed + 1))
      done
      echo "$fs round $round: $checked runs checked, $failed failed"
      [ "$checked" -gt 0 ] && [ "$failed" -eq 0 ]
    '';
  };

  machine = disks: {
    boot = {
      kernelModules = [
        "dm-log-writes"
        "dm-snapshot"
      ];
      supportedFilesystems = filesystems;
    };
    # The file systems bring their own tools.
    environment.systemPackages = [
      lvm2
      power-loss
      replay
      replay-log
      smoke-check
      smoke-run
    ];
    virtualisation = {
      cores = 2;
      emptyDiskImages = disks;
      memorySize = 2048;
    };
  };

  disk = serial: size: extra: {
    inherit size;
    driveConfig = lib.recursiveUpdate { deviceExtraOpts.serial = serial; } extra;
  };

  # dm-log-writes records the guest's flushes above the virtual disk, and a
  # replay never outlives its VM, so the host need not make them durable.
  scratch = serial: size: disk serial size { driveExtraOpts.cache = "unsafe"; };
in
{
  # ⚠️ QEMU's `quit`, which `crash()` sends, completes and flushes the writes its
  # virtual disks took from the guest: the crash loses the guest's page cache and
  # queued I/O, never a write a disk took. `replay` covers the writes a disk's own
  # cache loses or reorders.
  smoke = testers.runNixOSTest {
    name = "toshokan-power-loss-smoke";
    nodes.machine = machine (map (fs: disk fs 1024 { driveExtraOpts.cache = "none"; }) filesystems);
    testScript = ''
      import time

      filesystems = ${builtins.toJSON filesystems}
      mkfs = ${builtins.toJSON mkfs}

      def mount_all():
          for fs in filesystems:
              machine.succeed(f"mkdir -p /mnt/{fs} && mount /dev/disk/by-id/virtio-{fs} /mnt/{fs}")

      machine.wait_for_unit("multi-user.target")
      for fs in filesystems:
          machine.succeed(f"{mkfs[fs]} /dev/disk/by-id/virtio-{fs}")
      for round, seconds in enumerate([2, 5, 11]):
          mount_all()
          for fs in filesystems:
              machine.succeed(f"systemd-run --unit power-loss-{fs} power-loss-smoke-run {fs} {round}")
          time.sleep(seconds)
          machine.crash()
          machine.start()
          machine.wait_for_unit("multi-user.target")
          mount_all()
          for fs in filesystems:
              print(machine.succeed(f"power-loss-smoke-check {fs} {round} 2>&1"))
          for fs in filesystems:
              machine.succeed(f"umount /mnt/{fs}")
    '';
  };

  replay = lib.genAttrs filesystems (
    fs:
    testers.runNixOSTest {
      name = "toshokan-power-loss-${fs}";
      nodes.machine = lib.recursiveUpdate (machine [
        (scratch "data" (512 * workloads))
        (scratch "log" (4096 * workloads))
        (scratch "replay" (512 * workloads))
        (scratch "cow" (512 * workloads))
      ]) { virtualisation = replayMachine; };
      testScript = ''
        machine.wait_for_unit("multi-user.target")
        workloads = machine.succeed("power_loss list").split()[::2]
        assert len(workloads) <= ${toString workloads}, f"{len(workloads)} workloads, slices for ${toString workloads}"
        status, out = machine.execute(
            f"power-loss-replay ${fs} '${mkfs.${fs}}' ${toString samples} 1 {' '.join(workloads)} 2>&1", timeout=None
        )
        print(out)
        assert status == 0, "crash states failed their checks"
      '';
    }
  );
}
