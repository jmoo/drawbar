# The power-loss workloads and checker, bound for a page.
{
  commonArgs,
  craneLib,
  lld,
  wasm-bindgen-cli,
  workspace,
}:
let
  args = commonArgs // {
    CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
    cargoExtraArgs = "--locked -p toshokan --features web";
    # ⚠️ nixpkgs' rustc links wasm with the system `lld` and bundles none.
    nativeBuildInputs = [ lld ];
    pname = "toshokan-browser-crash";
    inherit (workspace.manifests.toshokan) version;
  };
in
craneLib.mkCargoDerivation (
  args
  // {
    buildPhaseCargoCommand = "cargo build --release $cargoExtraArgs --example browser_crash";
    cargoArtifacts = craneLib.buildDepsOnly args;
    doInstallCargoArtifacts = false;
    installPhaseCommand = ''
      wasm-bindgen --target web --no-typescript --out-dir "$out" \
        target/wasm32-unknown-unknown/release/examples/browser_crash.wasm
    '';
    nativeBuildInputs = args.nativeBuildInputs ++ [ wasm-bindgen-cli ];
  }
)
