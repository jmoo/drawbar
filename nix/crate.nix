# One workspace crate, built and tested on its declared test features. Further
# arguments reach crane.
{
  cargoArtifacts,
  commonArgs,
  craneLib,
  workspace,
}:
{ crate, ... }@args:
craneLib.buildPackage (
  commonArgs
  // {
    inherit cargoArtifacts;
    cargoExtraArgs = "--locked -p ${crate}";
    cargoTestExtraArgs = workspace.featureArgs (workspace.testFeaturesFor crate);
    pname = crate;
    inherit (workspace.manifests.${crate}) version;
  }
  // builtins.removeAttrs args [ "crate" ]
)
