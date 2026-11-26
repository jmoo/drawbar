# What a target's spec means to cargo.
{ lib }:
let
  inherit (lib)
    concatMapStringsSep
    concatStringsSep
    hasPrefix
    optional
    optionalAttrs
    toUpper
    ;

  # Where cargo puts the artifacts, and what BUILD_INFO records.
  tripleOf = spec: spec.triple or spec.crossPkgs.stdenv.hostPlatform.rust.rustcTarget;

  # CARGO_TARGET_<TRIPLE>_LINKER wants the triple upper-cased with underscores.
  envTriple = triple: toUpper (builtins.replaceStrings [ "-" ] [ "_" ] triple);

  # ⚠️ ELF and PE need the cross linker; wasm needs rustc's lld. Cross clang rejects
  # wasm's lld arguments with the misleading error `no such file or directory: wasm`.
  needsLinker = spec: spec ? crossPkgs && !hasPrefix "wasm" (tripleOf spec);
in
{
  inherit needsLinker tripleOf;

  # A target's linker and extra static libs reach cargo through the environment, not
  # through the cc wrapper.
  targetEnv =
    spec:
    optionalAttrs (needsLinker spec) (
      let
        inherit (spec.crossPkgs.stdenv) cc;
        triple = tripleOf spec;
      in
      {
        # rustc needs a real linker for a foreign target; the cross set's cc is it.
        "CARGO_TARGET_${envTriple triple}_LINKER" = "${cc}/bin/${cc.targetPrefix}cc";
        # Build scripts that compile C, such as ring's, need the target's compiler and
        # archiver too, or the `cc` crate builds host objects the target linker skips.
        "CC_${triple}" = "${cc}/bin/${cc.targetPrefix}cc";
        "AR_${triple}" = "${cc.bintools.bintools}/bin/${cc.targetPrefix}ar";
      }
    )
    // optionalAttrs (spec ? libs || spec.static or false) (
      let
        paths = concatMapStringsSep " " (l: "-L native=${l}/lib") spec.libs or [ ];
      in
      {
        # Put static libs on rustc's path directly; the cc wrapper can miss rustc's own
        # `-l:` requests.
        "CARGO_TARGET_${envTriple (tripleOf spec)}_RUSTFLAGS" = concatStringsSep " " (
          optional (spec.static or false) "-C target-feature=+crt-static" ++ optional (spec ? libs) paths
        );
      }
      // optionalAttrs (spec ? libs) {
        # rustdoc links doctests without the target's RUSTFLAGS; a cross build's
        # doctests are all the target's.
        RUSTDOCFLAGS = paths;
      }
    )
    // optionalAttrs (spec ? testRunner) {
      # cargo hands each test binary to this runner, which is what lets a foreign
      # target's tests run here.
      "CARGO_TARGET_${envTriple (tripleOf spec)}_RUNNER" = spec.testRunner.cmd;
    };
}
