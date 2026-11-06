# toshokan's library killed partway through the power-loss workloads in real
# browsers, and the origin's quota met. `sweep` runs the driver by hand; the check
# runs a slice of it in headless Chromium and Firefox.
{
  playwright-driver,
  procps,
  python3,
  runCommand,
  stdenv,
  wasm,
  writeShellApplication,
}:
let
  site = runCommand "toshokan-browser-crash-site" { } ''
    mkdir -p "$out"
    cp ${wasm}/* "$out/"
    cp ${./index.html} "$out/index.html"
    cp ${./harness.js} "$out/harness.js"
    cp ${./worker.js} "$out/worker.js"
  '';

  # Headless Chromium runs the headless shell; WebKit runs toshokan only on macOS.
  browsers = playwright-driver.browsers.override {
    withChromium = false;
    withFfmpeg = false;
    withWebkit = stdenv.hostPlatform.isDarwin;
  };

  sweep = writeShellApplication {
    name = "toshokan-browser-crash";
    runtimeEnv.PLAYWRIGHT_BROWSERS_PATH = browsers;
    runtimeInputs = [
      procps
      (python3.withPackages (python: [ python.playwright ]))
    ];
    text = ''exec python3 ${./driver.py} ${site} "$@"'';
  };
in
runCommand "toshokan-browser-crash"
  {
    nativeBuildInputs = [ sweep ];
    passthru = { inherit site sweep; };
    meta.description = "toshokan's library killed mid-change, and out of quota, in headless browsers";
  }
  ''
    export HOME="$TMPDIR"
    toshokan-browser-crash --stride 5
    toshokan-browser-crash --quota 4
    touch "$out"
  ''
