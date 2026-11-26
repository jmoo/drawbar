# drawbar for the browser. Native rustc supplies bare wasm32; wasm-bindgen pairs
# drawbar/WebUSB with the page.
# ⚠️ wasm-bindgen's CLI must match the crate pin; move the Cargo pin when they diverge.
{
  binaryen,
  commonArgs,
  craneLib,
  lld,
  wasm-bindgen-cli,
  workspace,
}:
let
  inherit (workspace.manifests.drawbar) version;

  args = commonArgs // {
    CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
    # The size-tuned profile from the workspace manifest.
    CARGO_PROFILE = "web";
    # ⚠️ `--lib` prevents the same-named binary stub from winning `drawbar.wasm`
    # and producing a package with no exports.
    cargoExtraArgs = "--locked -p drawbar --lib";
    # A wasm module runs in a browser, not here.
    doCheck = false;
    # ⚠️ nixpkgs' rustc links wasm with the system `lld` and bundles none;
    # without it the cdylib fails to link.
    nativeBuildInputs = [ lld ];
    pname = "drawbar-web";
    inherit version;
  };
in
craneLib.buildPackage (
  args
  // {
    cargoArtifacts = craneLib.buildDepsOnly args;

    installPhaseCommand = ''
      mkdir -p "$out"
      wasm-bindgen --target web --no-typescript --out-dir "$out/pkg" \
        target/wasm32-unknown-unknown/web/drawbar.wasm

      # ⚠️ Pages serves this gzipped, and inlining a function into its only caller
      # shrinks the file but grows the gzip. Data starts at 1 MiB and the stack grows
      # down from it, so only an overflowing stack reaches the low KiB that
      # `--low-memory-unused` assumes is unused. rustc emits no `target_features`
      # section, so the two features its output uses are named here.
      wasm-opt --enable-bulk-memory-opt --enable-nontrapping-float-to-int \
        --one-caller-inline-max-function-size=0 -Oz --converge --low-memory-unused \
        --strip-producers -o optimized.wasm "$out/pkg/drawbar_bg.wasm"
      mv optimized.wasm "$out/pkg/drawbar_bg.wasm"

      # Pages caches for ten minutes, so a cached glue file can meet a freshly
      # fetched module; the content hash in each name keeps that pair unreachable.
      wasm="drawbar_bg-$(sha256sum "$out/pkg/drawbar_bg.wasm" | cut -c-16).wasm"
      mv "$out/pkg/drawbar_bg.wasm" "$out/pkg/$wasm"
      substituteInPlace "$out/pkg/drawbar.js" --replace-fail drawbar_bg.wasm "$wasm"

      js="drawbar-$(sha256sum "$out/pkg/drawbar.js" | cut -c-16).js"
      mv "$out/pkg/drawbar.js" "$out/pkg/$js"

      # The page fetches the module itself to report progress, so it needs the
      # hashed name and the byte count the renamed file actually has.
      substitute ${./index.html} "$out/index.html" \
        --replace-fail pkg/drawbar.js "pkg/$js" \
        --replace-fail pkg/drawbar_bg.wasm "pkg/$wasm" \
        --replace-fail @version@ "${version}" \
        --replace-fail @wasmBytes@ "$(stat -c %s "$out/pkg/$wasm")"
      cp ${./favicon.svg} "$out/favicon.svg"
      # The demo sounds the welcome sheet fetches, beside the app that reads them.
      cp -r --no-preserve=mode ${../nord-format/tests/fixtures/demo} "$out/demo"

      if grep -qF drawbar_bg.wasm "$out/pkg/$js"; then
        echo "an unhashed asset name survived the rewrite" >&2
        exit 1
      fi

      for stale in pkg/drawbar.js pkg/drawbar_bg.wasm @version@ @wasmBytes@; do
        if grep -qF "$stale" "$out/index.html"; then
          echo "index.html still holds $stale after the rewrite" >&2
          exit 1
        fi
      done

      # The page imports `start` from the module; a bundle without it loads
      # and does nothing.
      grep -q 'function start' "$out/pkg/$js" || {
        echo "wasm-bindgen output lacks the page's entry point" >&2
        exit 1
      }
    '';

    meta.description = "drawbar built for the browser";
    nativeBuildInputs = args.nativeBuildInputs ++ [
      binaryen
      wasm-bindgen-cli
    ];

    # ⚠️ Dependency artifacts contain a dummy `drawbar.wasm`; clear it before
    # binding so only the real module can satisfy the install.
    preBuild = ''
      find "target/wasm32-unknown-unknown/web" -maxdepth 1 -type f -delete
    '';
  }
)
