# Cross-compiled Rust builds.
#
# `nord-usb` targets macOS, Linux, Windows and the browser (WebUSB), so all four
# need to stay compiling. nixpkgs' rustc ships only the host's std plus wasm32, so
# the toolchain comes from fenix, which can add per-target `rust-std`.
#
# Foreign *binaries* can't be executed here, so these are build-only checks; the
# native build (`pkgs.<crate>`, via mkRustCrates) is what actually runs the tests.
#
# Darwin cross-compilation is deliberately absent: producing macOS binaries needs the
# macOS SDK and is only sane on a macOS builder. So no single host covers all four —
# darwin gives {darwin, wasm32, windows} and linux gives {linux, wasm32, windows},
# and the union is the full matrix. That split is inherent, not a shortcoming here.
{ inputs, ... }:
{
  perSystem =
    { pkgs, system, ... }:
    let
      inherit (pkgs) lib;

      fenixPkgs = inputs.fenix.packages.${system};
      workspace = ../crates;
      version = (lib.importTOML (workspace + "/Cargo.toml")).workspace.package.version;

      # Targets reachable by cross-compiling from *any* host. The host's own
      # platform is covered by the ordinary native build instead.
      portableTargets = {
        wasm32 = {
          triple = "wasm32-unknown-unknown";
          # nusb is meaningless in a browser; the WebUSB backend replaces it.
          cargoFlags = [
            "--no-default-features"
            "--features"
            "web"
          ];
        };

        windows = {
          triple = "x86_64-pc-windows-gnu";
          cargoFlags = [ ];
          # rustc needs a real linker for this target; mingw-w64's gcc is it.
          cc = pkgs.pkgsCross.mingwW64.stdenv.cc;
          # rustc's windows-gnu target links `-l:libpthread.a`, which nixpkgs ships
          # in a separate package from the gcc wrapper. Without this the link dies
          # with "cannot find -l:libpthread.a" — only when actually producing a
          # binary, so the lib-only crates hide it.
          libs = [ pkgs.pkgsCross.mingwW64.windows.pthreads ];
        };
      };

      # Targets that only make sense from a particular host: the *other* CPU
      # architecture of the platform we are already on. Cross-OS is not possible in
      # both directions — Apple's SDK means darwin binaries can only be produced on
      # darwin — but cross-*arch* within one OS is cheap and worth having, so each
      # platform ends up fully covered rather than just the host's own chip.
      hostTargets =
        lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
          # Intel Macs. The native clang/SDK already links both arches.
          darwin-x86_64.triple = "x86_64-apple-darwin";
          darwin-x86_64.cargoFlags = [ ];
        }
        // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          # aarch64 Linux — meerkat (Asahi) runs this.
          linux-aarch64 = {
            triple = "aarch64-unknown-linux-gnu";
            cargoFlags = [ ];
            cc = pkgs.pkgsCross.aarch64-multiplatform.stdenv.cc;
          };
        };

      targets = portableTargets // hostTargets;

      # CARGO_TARGET_<TRIPLE>_LINKER wants the triple upper-cased with underscores.
      envTriple = triple: lib.toUpper (builtins.replaceStrings [ "-" ] [ "_" ] triple);

      mkCross =
        crate: name: spec:
        let
          toolchain = fenixPkgs.combine [
            fenixPkgs.stable.cargo
            fenixPkgs.stable.rustc
            fenixPkgs.targets.${spec.triple}.stable.rust-std
          ];
          rustPlatform = pkgs.makeRustPlatform {
            cargo = toolchain;
            rustc = toolchain;
          };
        in
        pkgs.stdenv.mkDerivation (
          {
            pname = "${crate}-${name}";
            inherit version;
            src = workspace;

            cargoDeps = rustPlatform.importCargoLock { lockFile = workspace + "/Cargo.lock"; };
            nativeBuildInputs = [
              toolchain
              rustPlatform.cargoSetupHook
            ]
            ++ lib.optional (spec ? cc) spec.cc;

            buildPhase = ''
              runHook preBuild
              cargo build --release --offline \
                --target ${spec.triple} -p ${crate} \
                ${lib.escapeShellArgs spec.cargoFlags}
              runHook postBuild
            '';

            installPhase = ''
              runHook preInstall
              mkdir -p "$out"
              rel="target/${spec.triple}/release"

              # Keep real artifacts, drop cargo's bookkeeping and intermediates.
              # Unix executables have no extension, so match by mode as well as by
              # name — matching only on extension silently produced an empty output
              # for the darwin/linux CLI builds, which then "passed".
              find "$rel" -maxdepth 1 -type f \
                \( -name '*.rlib' -o -name '*.rmeta' -o -name '*.a' -o -name '*.wasm' \
                   -o -name '*.exe' -o -name '*.dll' -o -name '*.so' -o -name '*.dylib' \
                   -o -perm -u+x \) \
                ! -name '*.d' \
                -exec cp -t "$out/" {} +

              # Refuse to install nothing. A cross build that produces no artifact is
              # a failure, not a pass — that is the whole point of these derivations.
              if [ -z "$(find "$out" -maxdepth 1 -type f -print -quit)" ]; then
                echo "no artifacts found in $rel — the build produced nothing" >&2
                ls -la "$rel" >&2 || true
                exit 1
              fi

              echo "${crate} ${version} built for ${spec.triple}" > "$out/BUILD_INFO"
              echo "installed:" >&2
              ls -la "$out" >&2
              runHook postInstall
            '';

            meta.description = "${crate} cross-compiled for ${spec.triple}";
          }
          // lib.optionalAttrs (spec ? cc) {
            "CARGO_TARGET_${envTriple spec.triple}_LINKER" = "${spec.cc}/bin/${spec.cc.targetPrefix}cc";
          }
          // lib.optionalAttrs (spec ? libs) {
            # Put the target's static libs on rustc's search path directly. Going
            # through buildInputs would leave it to the cc wrapper, which does not
            # reliably reach rustc's own `-l:` requests.
            "CARGO_TARGET_${envTriple spec.triple}_RUSTFLAGS" = lib.concatMapStringsSep " " (
              l: "-L native=${l}/lib"
            ) spec.libs;
          }
        );

      # Which crates get cross-built where. `nord-cli` is worth doing for Windows
      # because it produces a real `nord.exe` rather than an rlib — a much stronger
      # signal that the toolchain and linker are genuinely wired up. It has no
      # browser story, so no wasm32.
      crateTargets = {
        nord-usb = [ "wasm32" ] ++ lib.filter (t: t != "wasm32") (lib.attrNames targets);
        # A CLI has no browser story, so every target except wasm32.
        nord-cli = lib.filter (t: t != "wasm32") (lib.attrNames targets);
      };

      crossed = lib.concatMapAttrs (
        crate: names:
        lib.listToAttrs (map (n: lib.nameValuePair "${crate}-${n}" (mkCross crate n targets.${n})) names)
      ) crateTargets;

      # Actually *run* each foreign binary. Cross-compiling only proves it linked;
      # these prove it executes and behaves. That matters most for Windows, where
      # there is no machine in the loop at all — Wine runs a PE natively on
      # x86_64-linux, and qemu-user runs the aarch64 ELF, so both get real coverage
      # without the corresponding hardware.
      #
      # Linux-only: these emulators are not a sane proposition on aarch64-darwin.
      # The read-only inventory sweep, replayed. Exercises transport → wire → session
      # → op → CLI without a device, so the same proof runs on every target.
      pocScript = ../crates/nord-usb/tests/fixtures/inventory.script;

      # Execute the protocol test suite on an actual wasm VM.
      #
      # `wasm32-unknown-unknown` can only be *built* here — running it needs a browser
      # (that is where WebUSB lives). `wasm32-wasip1` runs the same protocol code under
      # wasmtime, so the logic gets real runtime coverage on a wasm target even though
      # the browser backend itself cannot be exercised in CI.
      wasmRuntimeTest =
        let
          triple = "wasm32-wasip1";
          toolchain = fenixPkgs.combine [
            fenixPkgs.stable.cargo
            fenixPkgs.stable.rustc
            fenixPkgs.targets.${triple}.stable.rust-std
          ];
          rustPlatform = pkgs.makeRustPlatform {
            cargo = toolchain;
            rustc = toolchain;
          };
        in
        pkgs.stdenv.mkDerivation {
          pname = "nord-usb-wasm-runtime";
          inherit version;
          src = workspace;
          cargoDeps = rustPlatform.importCargoLock { lockFile = workspace + "/Cargo.lock"; };
          nativeBuildInputs = [
            toolchain
            rustPlatform.cargoSetupHook
            pkgs.wasmtime
            # Build scripts and proc-macros still compile for the host.
            pkgs.stdenv.cc
          ];
          buildPhase = ''
            runHook preBuild
            # wasmtime wants somewhere to cache compiled modules; the sandbox has no
            # HOME, so give it one rather than letting it fail on /homeless-shelter.
            export HOME="$TMPDIR/home"
            mkdir -p "$HOME"
            export CARGO_TARGET_WASM32_WASIP1_RUNNER=wasmtime
            cargo test --release --offline -p nord-usb \
              --no-default-features --features replay --target ${triple} 2>&1 | tee test.log
            runHook postBuild
          '';
          installPhase = ''
            runHook preInstall
            grep -q '0 failed' test.log || { echo "wasm test run reported failures"; exit 1; }
            mkdir -p "$out"; cp test.log "$out/"
            runHook postInstall
          '';
          meta.description = "nord-usb protocol suite executed on a wasm VM";
        };

      # The command tree, asserted rather than described. Every noun's verb list is
      # pinned exactly, so a rename shows up as a failing check instead of a surprise —
      # which is the mechanism that makes a later compatibility promise enforceable.
      #
      # `raw` is deliberately absent from the top-level list and deliberately still
      # runs: hidden is not deprecated, it is the escape hatch class-generalisation
      # earns, and it has to stay tested to stay usable.
      slotVerbs = "get put move rename duplicate delete select info deps";
      surface = {
        "" = "inspect verify device program setlist help";
        "device" = "status info help";
        "program" = "${slotVerbs} edit help";
        "setlist" = "${slotVerbs} help";
        "raw" = "${slotVerbs} help";
      };

      surfaceChecks = ''
        echo
        echo "== the command surface =="
        # Clap lists each command as two spaces, the name, then its description.
        # Continuation lines are indented further, so they do not match.
        commands() { sed -n 's/^  \([a-z][a-z-]*\)  \+.*/\1/p' "$1" | tr '\n' ' '; }

        ${lib.concatStringsSep "\n" (
          lib.mapAttrsToList (noun: verbs: ''
            run ${noun} --help > "surface${if noun == "" then "-top" else "-${noun}"}.txt" 2>err.txt || {
              echo "nord ${noun} --help failed:"; cat err.txt; exit 1;
            }
            got=$(commands "surface${if noun == "" then "-top" else "-${noun}"}.txt")
            [ "$got" = "${verbs} " ] || {
              echo "nord ${noun}: command list drifted"
              echo "  want: ${verbs}"
              echo "  got:  $got"
              exit 1
            }
          '') surface
        )}

        # The escape hatch is reachable but unadvertised.
        if grep -q ' raw ' surface-top.txt; then
          echo "nord raw is meant to be hidden from the top-level help"; exit 1
        fi
        echo "ok: every noun's verb list matches"
      '';

      # `edit` needs no instrument and no corpus: a fresh default program is a legal
      # `.ne5p`, so the whole contract — one field changes, its bytes and the checksum
      # move, nothing else does, and the decode reports the new value — runs in CI on
      # every target.
      editCheck = ''
        echo
        echo "== nord program edit =="
        run program edit --set center_panel.gain=64 -o base.ne5p >/dev/null 2>err.txt || {
          echo "writing a default program failed:"; cat err.txt; exit 1;
        }
        # ⚠️ Capture to a file and grep the file — never pipe an emulated binary straight
        # into `grep`. Under Wine that pipeline reports no match for a line the captured
        # output plainly contains, and the failure branch cannot show it to you either.
        # Cause not established; the file is the reliable form and costs nothing.
        run verify base.ne5p > verified.txt 2>err.txt || {
          echo "a written program did not round-trip:"; cat verified.txt err.txt; exit 1;
        }
        cat verified.txt

        run program edit base.ne5p \
          --set center_panel.transpose=-5 \
          --set center_panel.transpose_enabled=true \
          -o edited.ne5p > edited.txt 2>err.txt || {
          echo "edit failed:"; cat err.txt; exit 1;
        }
        cat edited.txt

        # `transpose_enabled` is bit 23 and `transpose` bits 24..=27 of a panel starting
        # at 0x2e, so bytes 0x30 and 0x31 — plus the body CRC at 0x18..=0x1b, which any
        # body change moves. `cmp -l` counts from one.
        # `cmp` reports a difference by exiting non-zero, which is the expected outcome
        # here, so its status must not end the script.
        moved=$( (cmp -l base.ne5p edited.ne5p || true) | awk '{print $1}' | tr '\n' ' ')
        [ "$moved" = "25 26 27 28 49 50 " ] || {
          echo "edit touched the wrong bytes"
          echo "  want: 25 26 27 28 49 50   (crc32, then transpose and its enable)"
          echo "  got:  $moved"
          exit 1
        }

        # The decode has to agree with the edit, or the write went somewhere unrelated.
        run inspect edited.ne5p > decoded.txt 2>err.txt || {
          echo "inspect failed on the edited file:"; cat err.txt; exit 1;
        }
        grep -q 'transpose: -5  (yes)' decoded.txt || {
          echo "the edited value did not come back out of the decode:"
          cat decoded.txt; exit 1;
        }

        # Presentation is gated on a TTY, and there is none here. This is the check the
        # byte-identical Wine/Linux result rests on: an escape sequence surviving a pipe
        # would make that comparison depend on the console, not on the decode.
        run program edit base.ne5p --set center_panel.gain=1 --dry-run > plain.txt 2>&1
        run --color=always program edit base.ne5p --set center_panel.gain=1 --dry-run \
          > colored.txt 2>&1
        esc=$(printf '\033')
        if grep -q "$esc" plain.txt; then
          echo "piped output carried ANSI escapes:"; cat -v plain.txt; exit 1
        fi
        if ! grep -q "$esc" colored.txt; then
          echo "--color=always emitted no escapes, so the check above proves nothing"
          exit 1
        fi

        run program edit --fields > fields.txt 2>err.txt || {
          echo "--fields failed:"; cat err.txt; exit 1;
        }
        grep -q '^center_panel.transpose ' fields.txt || {
          echo "--fields does not list the field --set just wrote"; exit 1;
        }
        echo "ok: edit moved exactly the bytes it named, and nothing else"
      '';

      mkRunCheck =
        {
          name,
          pkg,
          bin,
          emulator ? null,
          runner ? "",
          expectUsage,
        }:
        pkgs.runCommand "${name}-test"
          {
            nativeBuildInputs = lib.optional (emulator != null) emulator;
            meta.description = "Run ${bin} end to end${
              lib.optionalString (emulator != null) " under ${emulator.pname or "an emulator"}"
            }";
          }
          ''
            # Emulators want a writable HOME; wine additionally refuses a prefix it
            # does not own, so point both at the build directory.
            export HOME="$TMPDIR/home"
            export WINEPREFIX="$HOME/.wine"
            export WINEDEBUG=-all
            # No network in the sandbox — stop wineboot reaching for gecko/mono.
            export WINEDLLOVERRIDES="mscoree,mshtml="
            mkdir -p "$HOME"

            run() { ${runner} ${pkg}/${bin} "$@"; }

            echo "== ${bin} --help =="
            run --help > help.txt 2>err.txt || { echo "failed to run:"; cat err.txt; exit 1; }
            cat help.txt
            grep -q 'Usage: ${expectUsage}' help.txt || {
              echo "unexpected output — wanted 'Usage: ${expectUsage}'"; exit 1;
            }

            # The POC itself: a full read-only inventory sweep over a replayed
            # exchange. This is the proof that the protocol stack works on this
            # target, not merely that the binary starts.
            echo
            echo "== ${bin} device status --replay =="
            run device status --replay ${pocScript} > poc.txt 2>err.txt || {
              echo "device status failed:"; cat err.txt; exit 1;
            }
            cat poc.txt

            for want in pianos samples programs 'set lists' '380 / 400 slots' '141 blocks each'; do
              grep -q "$want" poc.txt || {
                echo "POC output missing '$want'"; cat poc.txt; exit 1;
              }
            done

            ${surfaceChecks}
            ${editCheck}

            echo
            echo "ok: ${bin} completed the read-only inventory sweep"
            mkdir -p "$out"; cp help.txt poc.txt fields.txt edited.txt "$out/"
          '';
    in
    {
      packages = crossed;

      # `nix flake check` keeps every target honest. The host's own platform is
      # covered by the native builds, which additionally run each crate's tests.
      checks =
        crossed
        // {
          nord-usb-native = pkgs.nord-usb;
          nord-format-native = pkgs.nord-format;
          nord-usb-wasm-runtime = wasmRuntimeTest;

          # The POC on the host's own platform, no emulator involved.
          nord-cli-native-poc = mkRunCheck {
            name = "nord-cli-native-poc";
            pkg = pkgs.nord-cli;
            bin = "bin/nord";
            expectUsage = "nord";
          };
        }
        // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          # No Windows machine exists in this setup, so Wine is the only way this
          # target is ever actually executed.
          nord-cli-windows-poc = mkRunCheck {
            name = "nord-cli-windows-poc";
            pkg = crossed.nord-cli-windows;
            bin = "nord.exe";
            emulator = pkgs.wine64;
            runner = "wine";
            expectUsage = "nord.exe";
          };
        }
        // lib.optionalAttrs (pkgs.stdenv.hostPlatform.system == "x86_64-linux") {
          # aarch64 Linux is what meerkat runs; qemu-user covers it from here.
          nord-cli-linux-aarch64-poc = mkRunCheck {
            name = "nord-cli-linux-aarch64-poc";
            pkg = crossed.nord-cli-linux-aarch64;
            bin = "nord";
            emulator = pkgs.qemu;
            runner = "qemu-aarch64";
            expectUsage = "nord";
          };
        };
    };
}
