# The targets this host cross-builds for, how it runs their executables, and which
# crates build for which.
{
  lib,
  lld,
  pkgsCross,
  qemu,
  stdenv,
  wasmtime,
}:
let
  inherit (lib) attrNames filter optionalAttrs;
  inherit (stdenv.hostPlatform) isLinux system;

  targets = {
    windows = {
      crossPkgs = pkgsCross.mingwW64;
      libs = [ pkgsCross.mingwW64.windows.pthreads ];
    };

    wasip1 = {
      cargoFlags = [
        "--no-default-features"
        "--features"
        "replay"
      ];
      crossPkgs = pkgsCross.wasi32;
      testRunner = {
        # The suite reads its scripts off the source tree, which a wasm sandbox
        # sees only through a preopened directory.
        cmd = "wasmtime --dir /";
        packages = [
          lld
          wasmtime
        ];
      };
    };
  }
  # ⚠️ A musl cross set links against the store's `ld-musl` unless told otherwise;
  # `static` links musl in, and the cross build refuses a binary that still names a
  # loader.
  // optionalAttrs isLinux {
    linux-aarch64 = {
      crossPkgs = pkgsCross.aarch64-multiplatform-musl;
      static = true;
    };
    linux-x86_64 = {
      crossPkgs = pkgsCross.musl64;
      static = true;
    };
  };
in
{
  inherit targets;

  # How this host runs a target's executables: an emulator, or null where it runs them
  # itself. A target missing here is built and never run.
  emulators =
    optionalAttrs (system == "x86_64-linux") {
      linux-aarch64 = {
        cmd = "qemu-aarch64";
        package = qemu;
      };
      linux-x86_64 = null;
    }
    // optionalAttrs (system == "aarch64-linux") {
      linux-aarch64 = null;
    };

  # Cross-build applications to exercise their whole dependency stack. `nord-usb`
  # also keeps wasip1 because its own suite executes there in a wasm VM, and
  # `toshokan` builds for Windows because its Windows renames compile only there.
  crateTargets = {
    # A CLI has no use on WASI.
    nord-cli = filter (t: t != "wasip1") (attrNames targets);
    nord-usb = [ "wasip1" ];
    drawbar = [ "windows" ];
    toshokan = [ "windows" ];
  };
}
