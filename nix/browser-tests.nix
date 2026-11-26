# Each crate's declared browser suites, in headless Chromium and Firefox.
{
  chromedriver,
  chromium,
  commonArgs,
  craneLib,
  firefox,
  geckodriver,
  lib,
  linkFarmFromDrvs,
  lld,
  wasm-bindgen-cli,
  workspace,
  writeText,
}:
let
  inherit (lib)
    concatMap
    escapeShellArgs
    filterAttrs
    getExe
    mapAttrsToList
    ;
  inherit (workspace) featureArgs manifests testFeaturesFor;

  suitesOf = manifest: manifest.metadata.nix.browserTests or [ ];

  suite =
    name: manifest:
    let
      # ⚠️ Only the suites' own build names them: the dependency build compiles
      # stand-in sources, which hold no test target.
      suites = escapeShellArgs (
        concatMap (suite: [
          "--test"
          suite
        ]) (suitesOf manifest)
      );
      args = commonArgs // {
        CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
        CARGO_PROFILE = "release-tests";
        cargoExtraArgs = "--locked -p ${name} ${featureArgs (testFeaturesFor name)}";
        nativeBuildInputs = [ lld ];
        pname = "${name}-browser-tests";
        inherit (manifest) version;
      };
    in
    craneLib.mkCargoDerivation (
      args
      // {
        CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER = "wasm-bindgen-test-runner";
        WASM_BINDGEN_TEST_TIMEOUT = "300";
        WASM_BINDGEN_TEST_WEBDRIVER_JSON = writeText "webdriver.json" (
          builtins.toJSON {
            "goog:chromeOptions" = {
              args = [
                "--disable-dev-shm-usage"
                "--no-sandbox"
              ];
              binary = getExe chromium;
            };
            "moz:firefoxOptions".binary = getExe firefox;
          }
        );
        buildPhaseCargoCommand = ''
          export HOME="$TMPDIR/home"
          mkdir -p "$HOME"
          CHROMEDRIVER=${getExe chromedriver} cargo test --profile release-tests $cargoExtraArgs ${suites}
          GECKODRIVER=${getExe geckodriver} cargo test --profile release-tests $cargoExtraArgs ${suites}
        '';
        cargoArtifacts = craneLib.buildDepsOnly args;
        doInstallCargoArtifacts = false;
        installPhaseCommand = ''touch "$out"'';
        nativeBuildInputs = args.nativeBuildInputs ++ [ wasm-bindgen-cli ];
      }
    );
in
linkFarmFromDrvs "browser-tests" (
  mapAttrsToList suite (filterAttrs (_: manifest: suitesOf manifest != [ ]) manifests)
)
