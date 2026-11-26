final: prev:
let
  inherit (final) lib;
  inherit (lib)
    concatMapAttrs
    filterAttrs
    genAttrs
    mapAttrs
    mapAttrs'
    nameValuePair
    optionalAttrs
    ;
  inherit (final.stdenv.hostPlatform) isDarwin isLinux;
in
{
  # Everything the project builds, and what builds it. Each file states its inputs
  # as arguments, which `callPackage` fills from this scope and then from nixpkgs.
  drawbarPackages = lib.makeScope final.newScope (
    self:
    let
      inherit (self) callPackage workspace;
    in
    {
      workspace = import ./nix/workspace.nix { inherit lib; };

      craneLib = lib.crane;
      commonArgs = import ./nix/common-args.nix {
        inherit lib;
        inherit (self) craneLib workspace;
      };
      inherit (callPackage ./crates/drawbar/native-libs.nix { }) audioArgs guiLibs;
      cargoArtifacts = callPackage ./nix/cargo-artifacts.nix { };
      mkCrate = callPackage ./nix/crate.nix { };

      # A crate with a `package.nix` builds itself; any other is `mkCrate`'s.
      crates = mapAttrs (
        crate: dir:
        if builtins.pathExists (dir + "/package.nix") then
          callPackage (dir + "/package.nix") { }
        else
          self.mkCrate { inherit crate; }
      ) workspace.dirs;

      nordInstallCheck = callPackage ./crates/nord-cli/install-check.nix { };

      inherit (callPackage ./nix/cross/targets.nix { }) crateTargets emulators targets;
      crossEnv = import ./nix/cross/env.nix { inherit lib; };
      mkCross = callPackage ./nix/cross/build.nix { };
      cross = mapAttrs (
        crate: names: genAttrs names (name: self.mkCross crate name self.targets.${name})
      ) self.crateTargets;

      tarballs = mapAttrs (
        target: spec: callPackage ./crates/nord-cli/tarball.nix { inherit spec target; }
      ) (filterAttrs (_: spec: spec.static or false) self.targets);

      # Every `<crate>-<target>` this host builds, the release tarballs and the web
      # bundle in one set, because the names are host-dependent and a consumer
      # enumerating them cannot write the list down.
      crossPackages =
        concatMapAttrs (crate: mapAttrs' (target: nameValuePair "${crate}-${target}")) self.cross
        // mapAttrs' (target: nameValuePair "nord-cli-${target}-tarball") self.tarballs
        // {
          inherit (self) drawbar-web;
        };

      tests = callPackage ./nix/tests.nix { };
      clippy = callPackage ./nix/clippy.nix { };
      rustdoc = callPackage ./nix/rustdoc.nix { };
      browser-tests = callPackage ./nix/browser-tests.nix { };

      drawbar-web = callPackage ./crates/drawbar/web.nix { };
      drawbar-web-launch = callPackage ./crates/drawbar/web-launch.nix { };
      drawbar-app = callPackage ./crates/drawbar/macos-app.nix { };

      # drawbar as one Linux executable: the tested binary, set to run on the host's
      # libraries, and the AppImage runtime.
      drawbar-appimage = callPackage ./crates/drawbar/appimage {
        inherit (self.crates) drawbar;
        inherit (workspace) repository;
      };

      # The power-loss workloads and checker in a page whose runs a driver kills, in
      # Chromium and Firefox; Linux only, as the other browser suites.
      toshokan-browser-crash = callPackage ./crates/toshokan/power-loss/browser {
        wasm = callPackage ./crates/toshokan/power-loss/browser/wasm.nix { };
      };

      # The portable-root protocol, model-checked: the small suite, and the deep one.
      toshokan-spec = callPackage ./crates/toshokan/spec { suite = "toshokan-spec"; };
      toshokan-spec-configs = callPackage ./crates/toshokan/spec/configs.nix { };
      toshokan-spec-deep = callPackage ./crates/toshokan/spec { suite = "toshokan-spec-deep"; };

      docs = callPackage ./nix/docs.nix { };
      site = callPackage ./nix/site.nix { web = self.drawbar-web; };

      # The corpus package supplies its library-filtered committed tier; `full` adds
      # the R2 vendor samples, untrimmed captures and bundles.
      corpus = final.callPackage (import ./nix/corpus.nix) { };
      corpus-full = self.corpus.override { full = true; };

      # The full tier is the committed suite pointed at the bigger assembly.
      corpusSuites = {
        committed = genAttrs workspace.corpusCrates (
          crate: callPackage ./nix/corpus-suite.nix { inherit crate; }
        );
        full = mapAttrs (
          _: suite:
          suite.override {
            corpus = self.corpus-full;
            tier = "corpus-full";
          }
        ) self.corpusSuites.committed;
      };

      # `all` excludes the corpus roll-ups; the R2 tier needs credentials this build
      # cannot assume.
      all = final.linkFarm "all" (
        self.crates
        // self.crossPackages
        // {
          inherit (self) docs;
        }
      );
      all-corpus = final.linkFarm "all-corpus" self.corpusSuites.committed;
      # ⚠️ R2 stays outside checks because it needs credentials or a store seeded
      # by `corpus nix-add`.
      all-corpus-full = final.linkFarm "all-corpus-full" self.corpusSuites.full;

      # Each crate's shipping build on its own features; `tests` runs the suites.
      shipping = final.linkFarm "packages" (
        mapAttrs (_: crate: crate.overrideAttrs { doCheck = false; }) self.crates
      );
    }
  );

  # The names flake.nix, CI and the scripts address the scope by.
  nord =
    let
      scope = final.drawbarPackages;
    in
    scope.crates
    // scope.crossPackages
    # Not in `all`: they start browsers, and only on Linux.
    // optionalAttrs isLinux {
      inherit (scope) browser-tests drawbar-appimage toshokan-browser-crash;
    }
    // optionalAttrs isDarwin { inherit (scope) drawbar-app; }
    // mapAttrs' (name: nameValuePair "${name}-corpus") scope.corpusSuites.committed
    // mapAttrs' (name: nameValuePair "${name}-corpus-full") scope.corpusSuites.full
    // {
      inherit (scope)
        all
        all-corpus
        all-corpus-full
        clippy
        corpus
        corpus-full
        crates
        crossPackages
        docs
        drawbar-web-launch
        guiLibs
        rustdoc
        site
        tests
        toshokan-spec
        toshokan-spec-configs
        toshokan-spec-deep
        ;
      inherit (scope.workspace) edition;
      inherit (final) rustfmt;

      packages = scope.shipping;

      # `nix run .#toshokan-browser-crash -- --stride 1`: the whole crash sweep,
      # with WebKit on macOS.
      toshokan-browser-crash-sweep = scope.toshokan-browser-crash.sweep;
    };
}
