# The workspace's suites and the CLI checks, as scripts/test.bash runs them.
{
  audioArgs,
  commonArgs,
  craneLib,
  jq,
  lib,
  workspace,
}:
let
  args =
    commonArgs
    // audioArgs
    // {
      CARGO_PROFILE = "release-tests";
      cargoExtraArgs = "--locked --workspace ${workspace.featureArgs workspace.testFeatures}";
      pname = "workspace-tests";
      version = "0";
    };
in
craneLib.mkCargoDerivation (
  args
  // {
    NORD_TEST_FEATURES = lib.concatStringsSep "," workspace.testFeatures;
    buildPhaseCargoCommand = "bash ${../scripts/test.bash}";
    cargoArtifacts = craneLib.buildDepsOnly args;
    doInstallCargoArtifacts = false;
    installPhaseCommand = ''touch "$out"'';
    nativeBuildInputs = (args.nativeBuildInputs or [ ]) ++ [ jq ];
  }
)
