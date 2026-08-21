final: prev:
let
  inherit (final.lib)
    attrNames
    cleanSourceWith
    concatMap
    concatMapAttrs
    concatMapStringsSep
    concatStringsSep
    crane
    escapeShellArgs
    filter
    genAttrs
    hasInfix
    hasPrefix
    hasSuffix
    importTOML
    listToAttrs
    makeLibraryPath
    makeOverridable
    mapAttrs
    mapAttrs'
    nameValuePair
    optional
    optionalAttrs
    optionals
    optionalString
    toUpper
    ;

  workspace = ./crates;

  workspaceManifest = (importTOML (workspace + "/Cargo.toml")).workspace;
  inherit (workspaceManifest) members;
  inherit (workspaceManifest.package) edition version;

  commonArgs = {
    inherit version;
    src = cleanSourceWith {
      name = "source";
      src = workspace;
      filter =
        path: type:
        !hasInfix "/nord-cli/checks" path
        && (crane.filterCargoSources path type || hasSuffix ".script" path || hasSuffix ".snapshot" path);
    };

    strictDeps = true;
  };

  # One dependency build, shared by every crate that follows. `drawbar` is left out on
  # purpose: it is the only crate carrying a GUI stack, and building `nord-format`
  # should not mean compiling eframe first.
  cargoArtifacts = crane.buildDepsOnly (
    commonArgs
    // {
      pname = "workspace";
      cargoExtraArgs = "--locked --workspace --exclude drawbar";
    }
  );

  # Every member's `[package]` table, keyed by the name cargo knows it by.
  manifests = listToAttrs (
    map (
      member:
      let
        crate = (importTOML (workspace + "/${member}/Cargo.toml")).package;
      in
      nameValuePair crate.name crate
    ) members
  );

  # Test-only Cargo features are declared by the crate itself, under
  # `[package.metadata.nix] testFeatures = [ … ]`, so adding a crate needs no wiring
  # here — only a `members` entry in the workspace manifest.
  testFeaturesFor = name: manifests.${name}.metadata.nix.testFeatures or [ ];

  featureArgs =
    features: optionalString (features != [ ]) "--features ${concatStringsSep "," features}";

  mkCrate = makeOverridable (
    { crate, ... }@args:
    crane.buildPackage (
      commonArgs
      // {
        inherit cargoArtifacts;
        cargoExtraArgs = "--locked -p ${crate}";
        cargoTestExtraArgs = featureArgs (testFeaturesFor crate);
        pname = crate;
      }
      // builtins.removeAttrs args [ "crate" ]
    )
  );

  # ⚠️ winit, glutin and xkbcommon reach for these with `dlopen`, so nothing in the
  # build records a dependency on them: an unwrapped binary links and installs
  # cleanly, then dies at startup with `NoWaylandLib`. Anything that runs the
  # native `drawbar` — the package, the dev shell — has to put them on the loader's
  # path itself.
  guiLibs = optionals final.stdenv.hostPlatform.isLinux (
    with final;
    [
      libGL
      libx11
      libxcursor
      libxi
      libxkbcommon
      libxrandr
      wayland
    ]
  );

  crates = mapAttrs (name: _: mkCrate { crate = name; }) manifests // {
    drawbar =
      let
        pkg = mkCrate {
          crate = "drawbar";
          meta.mainProgram = "drawbar";
        };
      in
      if guiLibs == [ ] then
        pkg
      else
        pkg.overrideAttrs (old: {
          nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ [ final.makeWrapper ];
          postInstall = ''
            wrapProgram "$out/bin/drawbar" \
              --prefix LD_LIBRARY_PATH : ${makeLibraryPath guiLibs}
          '';
        });

    # `nord-cli` additionally proves, before it is allowed to be a package at
    # all, that the binary it installed runs.
    nord-cli =
      (mkCrate {
        crate = "nord-cli";
        meta.mainProgram = "nord";
      }).overrideAttrs
        (pocInstallCheck {
          bin = "bin/nord";
        });
  };

  targets = {
    windows = {
      crossPkgs = final.pkgsCross.mingwW64;
      libs = [ final.pkgsCross.mingwW64.windows.pthreads ];
    };

    wasip1 = {
      crossPkgs = final.pkgsCross.wasi32;
      cargoFlags = [
        "--no-default-features"
        "--features"
        "replay"
      ];
      testRunner = {
        cmd = "wasmtime";
        packages = [
          final.lld
          final.wasmtime
        ];
      };
    };
  }
  // optionalAttrs final.stdenv.hostPlatform.isLinux {
    linux-aarch64.crossPkgs = final.pkgsCross.aarch64-multiplatform;
  };

  emulators =
    optionalAttrs final.stdenv.hostPlatform.isLinux {
      windows = {
        cmd = "wine";
        package = final.wine64;
      };
    }
    // optionalAttrs (final.stdenv.hostPlatform.system == "x86_64-linux") {
      linux-aarch64 = {
        cmd = "qemu-aarch64";
        package = final.qemu;
      };
    };

  # Where cargo puts the artifacts, and what BUILD_INFO records.
  tripleOf = spec: spec.triple or spec.crossPkgs.stdenv.hostPlatform.rust.rustcTarget;

  # CARGO_TARGET_<TRIPLE>_LINKER wants the triple upper-cased with underscores.
  envTriple = triple: toUpper (builtins.replaceStrings [ "-" ] [ "_" ] triple);

  # ⚠️ rustc drives a real linker for the ELF and PE targets, and its own lld for the
  # wasm ones. Hand a wasm target the cross set's clang instead and it chokes on lld's
  # arguments — "no such file or directory: 'wasm'" — rather than saying so.
  needsLinker = spec: spec ? crossPkgs && !hasPrefix "wasm" (tripleOf spec);

  # Where a target needs a linker or extra static libs, cargo learns about it through
  # the environment rather than through the cc wrapper.
  targetEnv =
    spec:
    optionalAttrs (needsLinker spec) {
      # rustc needs a real linker for a foreign target; the cross set's cc is it.
      "CARGO_TARGET_${envTriple (tripleOf spec)}_LINKER" =
        let
          inherit (spec.crossPkgs.stdenv) cc;
        in
        "${cc}/bin/${cc.targetPrefix}cc";
    }
    // optionalAttrs (spec ? libs) {
      # Put the target's static libs on rustc's search path directly. Going through
      # buildInputs would leave it to the cc wrapper, which does not reliably reach
      # rustc's own `-l:` requests.
      "CARGO_TARGET_${envTriple (tripleOf spec)}_RUSTFLAGS" = concatMapStringsSep " " (
        l: "-L native=${l}/lib"
      ) spec.libs;
    }
    // optionalAttrs (spec ? testRunner) {
      # cargo hands each test binary to this instead of executing it directly, which
      # is the whole difference between a target that is built and one that is run.
      "CARGO_TARGET_${envTriple (tripleOf spec)}_RUNNER" = spec.testRunner.cmd;
    };

  mkCross =
    crate: name: spec:
    let
      # Cargo is target-agnostic; rustc is not. Swapping only rustc keeps the build
      # native — the sandbox's own coreutils, no cross stdenv — while giving it a
      # compiler that holds the target's std.
      crossLib = crane.overrideScope (
        _: _: { rustc = if spec ? crossPkgs then spec.crossPkgs.buildPackages.rustc else final.rustc; }
      );

      args =
        commonArgs
        // {
          pname = "${crate}-${name}";
          CARGO_BUILD_TARGET = tripleOf spec;
          cargoExtraArgs = escapeShellArgs (
            [
              "--locked"
              "-p"
              crate
            ]
            ++ spec.cargoFlags or [ ]
          );
          # A foreign binary cannot be executed here unless the target brought
          # something that can run it.
          doCheck = spec ? testRunner;
          nativeBuildInputs =
            optional (needsLinker spec) spec.crossPkgs.stdenv.cc ++ spec.testRunner.packages or [ ];
        }
        // optionalAttrs (spec ? testRunner) {
          # wasmtime wants somewhere to cache compiled modules; the sandbox has no
          # HOME, so give it one rather than letting it fail on /homeless-shelter.
          preCheck = ''
            export HOME="$TMPDIR/home"
            mkdir -p "$HOME"
          '';
        }
        // targetEnv spec;

      pkg = crossLib.buildPackage (
        args
        // {
          cargoArtifacts = crossLib.buildDepsOnly args;

          # crane installs what cargo's build log calls a binary or a cdylib, and an
          # rlib is neither: left to it, the lib-only cross builds install nothing
          # and pass. Install by hand instead, and assert that something arrived.
          doNotPostBuildInstallCargoBinaries = true;

          # ⚠️ The inherited dependency artifacts were produced from crane's dummy
          # stand-ins for this workspace's own crates, so the target directory
          # already holds a file under the name this build is about to write. Only
          # workspace members land at that level — dependencies stay in `deps/` —
          # so clearing it costs nothing and keeps the install below from passing
          # on a stand-in.
          preBuild = ''
            find "target/${tripleOf spec}/release" -maxdepth 1 -type f -delete
          '';

          installPhaseCommand = ''
            mkdir -p "$out"
            rel="target/${tripleOf spec}/release"

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

            echo "${crate} ${version} built for ${tripleOf spec}" > "$out/BUILD_INFO"
            echo "installed:" >&2
            ls -la "$out" >&2
          '';

          meta.description = "${crate} cross-compiled for ${tripleOf spec}";
        }
      );
    in
    # The end-to-end run, on the only crate that produces a binary and only where
    # something here can execute it. `overrideAttrs`, so the deps build derived
    # from `args` above cannot inherit the check — the binary it installs is a
    # stub crane stood in for the workspace's crates, and answers nothing.
    if crate == "nord-cli" && emulators ? ${name} then
      pkg.overrideAttrs (pocInstallCheck {
        bin = if spec.crossPkgs.stdenv.hostPlatform.isWindows then "nord.exe" else "nord";
        emulator = emulators.${name};
      })
    else
      pkg;

  # Which crates get cross-built where: the applications, because a real binary
  # is the artifact worth shipping and a much stronger signal than an rlib that
  # the toolchain and linker are genuinely wired up — every library they depend
  # on is compiled for the target along the way. A library keeps a target of its
  # own only where it yields coverage no application build does: wasip1 is the
  # one place `nord-usb`'s own suite executes on a wasm VM.
  crateTargets = {
    # A CLI has no wasi story.
    nord-cli = filter (t: t != "wasip1") (attrNames targets);
    nord-usb = [ "wasip1" ];
  };

  # The browser build of drawbar, which is also what covers `nord-usb`'s WebUSB
  # backend: the application's wasm library, bound for the web by wasm-bindgen,
  # beside the page that loads it. The output is what a static file server
  # serves. Built by the native toolchain — nixpkgs has no cross set for bare
  # wasm32, there being no libc to build a stdenv from, and the ordinary rustc
  # ships that target's std. (`pkgsCross.wasi32` is a *different* target, not
  # this one under another name.)
  #
  # ⚠️ `wasm-bindgen-cli` must be the exact version of the workspace's
  # `wasm-bindgen` pin — the CLI refuses a module built by any other. The pin is
  # held at nixpkgs' CLI version, so if this build fails on a version mismatch,
  # move the pin in drawbar's Cargo.toml to what the CLI reports.
  drawbar-web =
    let
      args = commonArgs // {
        CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
        # The size-tuned profile from the workspace manifest.
        CARGO_PROFILE = "web";
        # ⚠️ `--lib` is load-bearing: the crate also has a binary target of the
        # same name, both write `drawbar.wasm`, and cargo does not define which
        # survives — when it is the binary's stub main, wasm-bindgen emits a
        # package that exports nothing.
        cargoExtraArgs = "--locked -p drawbar --lib";
        # A wasm module runs in a browser, not here.
        doCheck = false;
        # ⚠️ nixpkgs' rustc links wasm through the system `lld` rather than
        # carrying a bundled one; without it the cdylib dies at link time.
        nativeBuildInputs = [ final.lld ];
        pname = "drawbar-web";
      };
    in
    crane.buildPackage (
      args
      // {
        cargoArtifacts = crane.buildDepsOnly args;
        nativeBuildInputs = args.nativeBuildInputs ++ [ final.wasm-bindgen-cli ];

        # ⚠️ The inherited target directory holds a `drawbar.wasm` built from
        # crane's dummy stand-in for the crate. The real build overwrites it,
        # but if it ever did not, wasm-bindgen would bind the stub without
        # complaint — so clear it, leaving the real module the only candidate.
        preBuild = ''
          find "target/wasm32-unknown-unknown/web" -maxdepth 1 -type f -delete
        '';

        installPhaseCommand = ''
          mkdir -p "$out"
          wasm-bindgen --target web --out-dir "$out/pkg" \
            target/wasm32-unknown-unknown/web/drawbar.wasm
          cp ${./crates/drawbar/index.html} "$out/index.html"

          # The page imports `start` from the module; a bundle without it loads
          # and does nothing.
          grep -q 'function start' "$out/pkg/drawbar.js" || {
            echo "wasm-bindgen output lacks the page's entry point" >&2
            exit 1
          }
        '';

        meta.description = "drawbar built for the browser";
      }
    );

  # One package per crate/target pair, named `<crate>-<target>` and exposed at the
  # top level beside the crates themselves. Which pairs exist depends on the host —
  # only a Mac can produce the Intel Mac binaries, only Linux the aarch64 Linux ones —
  # so a consumer takes the set rather than naming its members. The web bundle,
  # which every host builds, rides along.
  crossed =
    concatMapAttrs (
      crate: names:
      listToAttrs (map (n: nameValuePair "${crate}-${n}" (mkCross crate n targets.${n})) names)
    ) crateTargets
    // {
      inherit drawbar-web;
    };

  # The read-only inventory sweep, replayed. Exercises transport → wire → session
  # → op → CLI without a device, so the same proof runs on every target.
  pocScript = ./crates/nord-usb/tests/fixtures/inventory.script;

  # The nord-cli end-to-end run, as the package's own install check. The scripts
  # live with the CLI — crates/nord-cli/checks — and run against any built
  # binary; this shim only wires in the store paths and, for a foreign binary,
  # the emulator that can execute it here. Cross-compiling only proves the
  # binary linked; this proves it executes and behaves, Windows entirely under
  # Wine.
  pocInstallCheck =
    {
      bin,
      emulator ? null,
    }:
    {
      doInstallCheck = true;
      nativeInstallCheckInputs = optional (emulator != null) emulator.package;
      installCheckPhase = ''
        runHook preInstallCheck
        NORD_RUNNER=${optionalString (emulator != null) emulator.cmd} \
          POC_SCRIPT=${pocScript} \
          bash ${./crates/nord-cli/checks}/check.sh "$out/${bin}"
        runHook postInstallCheck
      '';
    };

  # The specimen corpus, and the suites that need it.
  #
  # ⚠️ The corpus is a private repo, so evaluating this overlay at all needs read
  # access to it.

  # ⚠️ The pinned rev lives on `size-tiering`, not the default branch, and
  # `fetchGit` only fetches the refs it is told about — without this it reports the
  # rev as not found. Drop it when the branch merges.
  corpusTree = builtins.fetchGit {
    ref = "size-tiering";
    rev = "43cfa477ac74a6e4f247ae97607b51f581b96aaf";
    url = "git+ssh://git@github.com/jmoo/nord-corpus.git";
  };

  # The corpus repo is the package, so what lands here is what that repo asserts
  # about itself: a model directory per instrument, its committed tier filtered
  # against `library.json`. `full` projects the R2 tier in on top — the vendor
  # sample pool, the untrimmed captures, the bundle archives.
  corpus = final.callPackage corpusTree { };
  corpusFull = corpus.override { full = true; };

  # The crates with a `corpus` feature. Both tiers and both roll-ups come from
  # this one list.
  corpusCrates = [
    "nord-format"
    "nord-usb"
  ];

  # One suite per corpus crate: the crate's own package — `final`'s, so a later
  # overlay changing a crate changes its suite — re-run by `.override` with the
  # specimens named and the feature that compiles the sweeps in.
  #
  # `NORD_CORPUS_DIR` is the Electro 5 tree and `NORD_CORPUS_ROOT` the corpus it
  # sits in. The depth suite reads the first, the whole-corpus sweep the second.
  committed = genAttrs corpusCrates (
    name:
    final.nord.crates.${name}.override {
      NORD_CORPUS_DIR = "${corpus}/ne5";
      NORD_CORPUS_ROOT = "${corpus}";
      cargoTestExtraArgs = featureArgs (testFeaturesFor name ++ [ "corpus" ]);
      pname = "${name}-corpus";
    }
  );

  # The full tier is the committed suite pointed at the bigger assembly.
  full = mapAttrs (
    name: suite:
    suite.override {
      NORD_CORPUS_DIR = "${corpusFull}/ne5";
      NORD_CORPUS_ROOT = "${corpusFull}";
      pname = "${name}-corpus-full";
    }
  ) committed;

in
{
  # One flat scope: every package by name — the crates, the cross builds, the
  # corpus suites — beside the roll-ups and the few non-package attrs the flake
  # reads.
  nord =
    crates
    // crossed
    // mapAttrs' (name: nameValuePair "${name}-corpus") committed
    // mapAttrs' (name: nameValuePair "${name}-corpus-full") full
    // {
      # Everything this host builds without reaching for the corpus. The corpus
      # roll-ups stay out: they are their own builds, and the R2 tier needs
      # credentials that `all` cannot assume.
      all = final.linkFarm "all" (crates // crossed);

      # Clippy over every crate and target, with each crate's test features on so the
      # tests are linted too. A warning fails it — this is `nix flake check`'s gate.
      clippy = crane.cargoClippy (
        commonArgs
        // {
          inherit cargoArtifacts;
          pname = "workspace-clippy";
          cargoExtraArgs = "--locked --workspace ${
            featureArgs (concatMap (name: map (f: "${name}/${f}") (testFeaturesFor name)) (attrNames manifests))
          }";
          cargoClippyExtraArgs = "--all-targets -- --deny warnings";
        }
      );

      all-corpus = final.linkFarm "all-corpus" committed;

      # ⚠️ Not among the checks: the R2 tier is a private bucket, so building this
      # needs either R2 credentials in the builder or a store seeded by
      # `corpus nix-add`, and `nix flake check` has to stay runnable with neither.
      all-corpus-full = final.linkFarm "all-corpus-full" full;

      # The corpus assemblies themselves.
      inherit corpus;
      corpus-full = corpusFull;

      # The workspace's own crates, keyed by the name cargo knows them by.
      inherit crates;

      # The `dlopen`ed display and GL libraries the native `drawbar` needs on the
      # loader's path; empty off Linux. The package wraps itself with them, the dev
      # shell exports them, so `cargo run -p drawbar` behaves like the package.
      inherit guiLibs;

      # The cross builds as a set, because their names are host-dependent and a
      # consumer enumerating them cannot write the list down.
      crossPackages = crossed;

      # `nix run .#drawbar-web`: serve the browser bundle on loopback and open it.
      drawbar-web-launch = final.writeShellApplication {
        name = "drawbar-web";
        runtimeInputs = [ final.miniserve ];
        text = ''
          port="''${DRAWBAR_PORT:-8080}"
          url="http://127.0.0.1:$port/"

          miniserve --index index.html --interfaces 127.0.0.1 --port "$port" \
            ${final.nord.drawbar-web} &
          server=$!
          trap 'kill "$server" 2>/dev/null || true' EXIT

          for _ in $(seq 40); do
            if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
              break
            fi
            sleep 0.25
          done

          if command -v open >/dev/null 2>&1; then
            open "$url"
          elif command -v xdg-open >/dev/null 2>&1; then
            xdg-open "$url"
          else
            echo "no opener found — open $url yourself" >&2
          fi

          wait "$server"
        '';
      };

      inherit edition;
      inherit (final) rustfmt;
    };
}
