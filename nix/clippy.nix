# Clippy over every crate and target, with each crate's test features and `corpus`
# on so every suite is linted, the corpus ones included; compiling them needs no
# specimens. A warning fails it.
{
  audioArgs,
  cargoArtifacts,
  commonArgs,
  craneLib,
  lib,
  workspace,
}:
let
  inherit (lib)
    attrNames
    concatMap
    elem
    optional
    ;
  inherit (workspace)
    corpusCrates
    featureArgs
    manifests
    testFeaturesFor
    ;
in
craneLib.cargoClippy (
  commonArgs
  // audioArgs
  // {
    inherit cargoArtifacts;
    cargoClippyExtraArgs = "--all-targets -- --deny warnings";
    cargoExtraArgs = "--locked --workspace ${
      featureArgs (
        concatMap (
          name: map (f: "${name}/${f}") (testFeaturesFor name ++ optional (elem name corpusCrates) "corpus")
        ) (attrNames manifests)
      )
    }";
    pname = "workspace-clippy";
    version = "0";
  }
)
