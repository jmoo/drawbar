# Every crate's public documentation. A warning, such as a link from a public item
# to a private one, fails it.
{
  audioArgs,
  cargoArtifacts,
  commonArgs,
  craneLib,
}:
craneLib.cargoDoc (
  commonArgs
  // audioArgs
  // {
    inherit cargoArtifacts;
    RUSTDOCFLAGS = "--deny warnings";
    cargoDocExtraArgs = "--no-deps";
    cargoExtraArgs = "--locked --workspace";
    pname = "workspace-rustdoc";
    version = "0";
  }
)
