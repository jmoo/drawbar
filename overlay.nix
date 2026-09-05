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
  inherit (workspaceManifest.package) edition;

  commonArgs = {
    src = cleanSourceWith {
      name = "source";
      src = workspace;
      filter =
        path: type:
        !hasInfix "/nord-cli/checks" path
        && (
          crane.filterCargoSources path type
          || hasSuffix ".script" path
          # The committed specimens and replay scripts, whatever their extensions.
          || hasInfix "/tests/fixtures/" path
          || hasInfix "/tests/scripts/" path
        );
    };

    strictDeps = true;
  };

  # Share one dependency build, excluding drawbar so library builds do not compile its GUI stack.
  cargoArtifacts = crane.buildDepsOnly (
    commonArgs
    // {
      pname = "workspace";
      version = "0";
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

  # Crates declare test-only features in `package.metadata.nix.testFeatures`, so new
  # workspace members need no overlay wiring.
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
        inherit (manifests.${crate}) version;
      }
      // builtins.removeAttrs args [ "crate" ]
    )
  );

  # ⚠️ winit, glutin and xkbcommon `dlopen` these: builds succeed without them, then
  # die with `NoWaylandLib`. Packages and the dev shell must set the loader path.
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

  audioArgs = optionalAttrs final.stdenv.hostPlatform.isLinux {
    buildInputs = [ final.alsa-lib ];
    nativeBuildInputs = [ final.pkg-config ];
  };

  crates = mapAttrs (name: _: mkCrate { crate = name; }) manifests // {
    drawbar =
      let
        pkg = mkCrate (
          {
            crate = "drawbar";
            meta.mainProgram = "drawbar";
          }
          // audioArgs
        );
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
        # The suite reads its scripts off the source tree, which a wasm sandbox
        # sees only through a preopened directory.
        cmd = "wasmtime --dir /";
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

  # ⚠️ ELF and PE need the cross linker; wasm needs rustc's lld. Cross clang rejects
  # wasm's lld arguments with the misleading error `no such file or directory: wasm`.
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
      # Put static libs on rustc's path directly; the cc wrapper can miss rustc's own
      # `-l:` requests.
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
      # Swap only rustc: sandbox tools stay native while the compiler supplies target std.
      crossLib = crane.overrideScope (
        _: _: { rustc = if spec ? crossPkgs then spec.crossPkgs.buildPackages.rustc else final.rustc; }
      );

      args =
        commonArgs
        // {
          pname = "${crate}-${name}";
          inherit (manifests.${crate}) version;
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

          # Crane skips rlibs; install lib-only cross artifacts manually and require output.
          doNotPostBuildInstallCargoBinaries = true;

          # ⚠️ Dependency artifacts include dummy workspace outputs at the target root.
          # Clear them so installation cannot pass on a stand-in; dependencies stay in `deps/`.
          preBuild = ''
            find "target/${tripleOf spec}/release" -maxdepth 1 -type f -delete
          '';

          installPhaseCommand = ''
            mkdir -p "$out"
            rel="target/${tripleOf spec}/release"

            # Keep real artifacts, not Cargo bookkeeping. Match mode too, because Unix
            # executables have no extension and an empty cross-build output can otherwise pass.
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

            echo "${crate} ${manifests.${crate}.version} built for ${tripleOf spec}" > "$out/BUILD_INFO"
            echo "installed:" >&2
            ls -la "$out" >&2
          '';

          meta.description = "${crate} cross-compiled for ${tripleOf spec}";
        }
      );
    in
    # Add the executable check only where a runner exists. `overrideAttrs` keeps it
    # off the dependency build, whose workspace binary is Crane's inert stand-in.
    if crate == "nord-cli" && emulators ? ${name} then
      pkg.overrideAttrs (pocInstallCheck {
        bin = if spec.crossPkgs.stdenv.hostPlatform.isWindows then "nord.exe" else "nord";
        emulator = emulators.${name};
      })
    else
      pkg;

  # Cross-build applications to exercise their whole dependency stack. `nord-usb`
  # also keeps wasip1 because its own suite executes there in a wasm VM.
  crateTargets = {
    # A CLI has no wasi story.
    nord-cli = filter (t: t != "wasip1") (attrNames targets);
    nord-usb = [ "wasip1" ];
  };

  # Native rustc supplies bare wasm32; wasm-bindgen pairs drawbar/WebUSB with the page.
  # ⚠️ Its CLI must match the crate pin; move the Cargo pin when they diverge.
  drawbar-web =
    let
      args = commonArgs // {
        CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
        # The size-tuned profile from the workspace manifest.
        CARGO_PROFILE = "web";
        # ⚠️ `--lib` prevents the same-named binary stub from winning `drawbar.wasm`
        # and producing a package with no exports.
        cargoExtraArgs = "--locked -p drawbar --lib";
        # A wasm module runs in a browser, not here.
        doCheck = false;
        # ⚠️ nixpkgs' rustc links wasm through the system `lld` rather than
        # carrying a bundled one; without it the cdylib dies at link time.
        nativeBuildInputs = [ final.lld ];
        pname = "drawbar-web";
        inherit (manifests.drawbar) version;
      };
    in
    crane.buildPackage (
      args
      // {
        cargoArtifacts = crane.buildDepsOnly args;
        nativeBuildInputs = args.nativeBuildInputs ++ [ final.wasm-bindgen-cli ];

        # ⚠️ Dependency artifacts contain a dummy `drawbar.wasm`; clear it before
        # binding so only the real module can satisfy the install.
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

  # Expose each host-supported `<crate>-<target>` package in one set, alongside
  # the host-independent web bundle.
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
  pocScript = ./crates/nord-usb/tests/scripts/device/inventory.script;

  # An editor-written Sample Editor project, for the file-verb edit check.
  pocProject = ./crates/nord-format/tests/fixtures/nsmpproj/one-zone.nsmpproj;

  # Run CLI checks against the installed binary, through an emulator when foreign.
  # This proves execution and behavior rather than linking alone.
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
          POC_PROJECT=${pocProject} \
          bash ${./crates/nord-cli/checks}/check.sh "$out/${bin}"
        runHook postInstallCheck
      '';
    };

  # ⚠️ Corpus suites fetch a private repo, so evaluating this overlay needs read access.

  corpusTree = builtins.fetchGit {
    ref = "device-facade-walks";
    rev = "f2bba8dec199ad02d1946fbfd15c67a5b972ab5a";
    url = "git+ssh://git@github.com/jmoo/nord-corpus.git";
  };

  # The corpus package supplies its library-filtered committed tier; `full` adds
  # the R2 vendor samples, untrimmed captures and bundles.
  corpus = final.callPackage corpusTree { };
  corpusFull = corpus.override { full = true; };

  # The crates with a `corpus` feature. Both tiers and both roll-ups come from
  # this one list.
  corpusCrates = [
    "nord-format"
    "nord-usb"
  ];

  # Override `final` crates with the corpus root and feature so later overlays
  # still flow into their suites.
  committed = genAttrs corpusCrates (
    name:
    final.nord.crates.${name}.override {
      NORD_CORPUS_ROOT = "${corpus}";
      cargoTestExtraArgs = featureArgs (testFeaturesFor name ++ [ "corpus" ]);
      pname = "${name}-corpus";
    }
  );

  # The full tier is the committed suite pointed at the bigger assembly.
  full = mapAttrs (
    name: suite:
    suite.override {
      NORD_CORPUS_ROOT = "${corpusFull}";
      pname = "${name}-corpus-full";
    }
  ) committed;

in
{
  # Crates, cross builds, corpus suites, roll-ups and flake metadata share one scope.
  nord =
    crates
    // crossed
    // mapAttrs' (name: nameValuePair "${name}-corpus") committed
    // mapAttrs' (name: nameValuePair "${name}-corpus-full") full
    // {
      # `all` excludes corpus roll-ups; the R2 tier needs credentials this build
      # cannot assume.
      all = final.linkFarm "all" (crates // crossed);

      # Clippy over every crate and target, with each crate's test features on so the
      # tests are linted too. A warning fails it — this is `nix flake check`'s gate.
      clippy = crane.cargoClippy (
        commonArgs
        // audioArgs
        // {
          inherit cargoArtifacts;
          pname = "workspace-clippy";
          version = "0";
          cargoExtraArgs = "--locked --workspace ${
            featureArgs (concatMap (name: map (f: "${name}/${f}") (testFeaturesFor name)) (attrNames manifests))
          }";
          cargoClippyExtraArgs = "--all-targets -- --deny warnings";
        }
      );

      all-corpus = final.linkFarm "all-corpus" committed;

      # ⚠️ R2 stays outside checks because it needs credentials or a store seeded
      # by `corpus nix-add`.
      all-corpus-full = final.linkFarm "all-corpus-full" full;

      # The corpus assemblies themselves.
      inherit corpus;
      corpus-full = corpusFull;

      # The workspace's own crates, keyed by the name cargo knows them by.
      inherit crates;

      # Native drawbar's `dlopen`ed display/GL libraries, empty off Linux. Package
      # wrapping and the dev shell give `cargo run` the same loader path.
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

          # ⚠️ Store paths carry epoch mtimes, so without this the browser's heuristic
          # freshness keeps a stale bundle for years and the glue no longer matches
          # the wasm. `no-cache` forces revalidation; the ETag still answers 304.
          miniserve --index index.html --interfaces 127.0.0.1 --port "$port" \
            --header "Cache-Control: no-cache" \
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
