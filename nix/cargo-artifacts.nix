# One dependency build shared by the library crates. It excludes drawbar so they do
# not compile its GUI stack; drawbar builds its own.
{ commonArgs, craneLib }:
craneLib.buildDepsOnly (
  commonArgs
  // {
    cargoExtraArgs = "--locked --workspace --exclude drawbar";
    pname = "workspace";
    version = "0";
  }
)
