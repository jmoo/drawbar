# One crate built for one target. A build that produces no artifact fails.
{
  commonArgs,
  craneLib,
  crossEnv,
  emulators,
  file,
  lib,
  nordInstallCheck,
  rustc,
  workspace,
}:
let
  inherit (lib)
    escapeShellArgs
    optional
    optionalAttrs
    optionalString
    ;
  inherit (crossEnv) needsLinker targetEnv tripleOf;
in
crate: name: spec:
let
  inherit (workspace.manifests.${crate}) version;

  # Swap only rustc: sandbox tools stay native while the compiler supplies target std.
  crossLib = craneLib.overrideScope (
    _: _: { rustc = if spec ? crossPkgs then spec.crossPkgs.buildPackages.rustc else rustc; }
  );

  args =
    commonArgs
    // {
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
        optional (needsLinker spec) spec.crossPkgs.stdenv.cc
        ++ optional (spec.static or false) file
        ++ spec.testRunner.packages or [ ];
      pname = "${crate}-${name}";
      inherit version;
    }
    // optionalAttrs (spec ? testRunner) {
      # wasmtime caches compiled modules under HOME, which the sandbox lacks;
      # without one it fails on /homeless-shelter.
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

        # A cross build that produces no artifact fails.
        if [ -z "$(find "$out" -maxdepth 1 -type f -print -quit)" ]; then
          echo "no artifacts found in $rel; the build produced nothing" >&2
          ls -la "$rel" >&2 || true
          exit 1
        fi

        ${optionalString (spec.static or false) ''
          # `file` calls a static position-independent executable `static-pie linked`.
          for bin in $(find "$out" -maxdepth 1 -type f -perm -u+x); do
            linkage="$(file -b "$bin")"
            case $linkage in
            *interpreter* | *dynamically\ linked*)
              echo "$bin is not static: $linkage" >&2
              exit 1
              ;;
            *statically\ linked* | *static-pie\ linked*) ;;
            *)
              echo "$bin: no linkage in $linkage" >&2
              exit 1
              ;;
            esac
          done
        ''}
        echo "${crate} ${version} built for ${tripleOf spec}" > "$out/BUILD_INFO"
        echo "installed:" >&2
        ls -la "$out" >&2
      '';

      meta.description = "${crate} cross-compiled for ${tripleOf spec}";

      # ⚠️ Dependency artifacts include dummy workspace outputs at the target root.
      # Clear them so installation cannot pass on a stand-in; dependencies stay in `deps/`.
      preBuild = ''
        find "target/${tripleOf spec}/release" -maxdepth 1 -type f -delete
      '';
    }
  );
in
# Add the executable check only where a runner exists. `overrideAttrs` keeps it
# off the dependency build, whose workspace binary is Crane's inert stand-in.
if crate == "nord-cli" && emulators ? ${name} then
  pkg.overrideAttrs (nordInstallCheck {
    bin = if spec.crossPkgs.stdenv.hostPlatform.isWindows then "nord.exe" else "nord";
    emulator = emulators.${name};
  })
else
  pkg
