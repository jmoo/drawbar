# The Rust toolchain and what hangs off it: a native build per workspace member,
# and the dev shell.
#
# The toolchain comes from fenix rather than nixpkgs because the cross builds in
# ./cross.nix need per-target `rust-std`, and one compiler for both keeps a native
# and a cross build from disagreeing about the same source.
{ inputs, ... }:
{
  perSystem =
    { pkgs, system, ... }:
    let
      inherit (pkgs) lib;

      workspace = ./crates;
      manifest = (lib.importTOML (workspace + "/Cargo.toml")).workspace;
      inherit (manifest) members;
      inherit (manifest.package) edition version;

      fenixPkgs = inputs.fenix.packages.${system};

      # `rustc` here carries the host std; the per-target `rust-std`s are what a
      # cross build adds. Nothing else belongs in a build's closure — rustfmt,
      # clippy and rust-analyzer are shell tools.
      mkToolchain =
        targets:
        fenixPkgs.combine (
          [
            fenixPkgs.stable.cargo
            fenixPkgs.stable.rustc
          ]
          ++ map (triple: fenixPkgs.targets.${triple}.stable.rust-std) targets
        );

      craneLibFor = targets: (inputs.crane.mkLib pkgs).overrideToolchain (mkToolchain targets);
      craneLib = craneLibFor [ ];

      # crane's own filter keeps only `.rs` and `.toml`. The decode snapshots and the
      # replay script are read at *test* time, so dropping them turns a source filter
      # into a runtime failure inside the sandbox.
      src = lib.cleanSourceWith {
        name = "source";
        src = workspace;
        filter =
          path: type:
          craneLib.filterCargoSources path type
          || lib.hasSuffix ".script" path
          || lib.hasSuffix ".snapshot" path;
      };

      commonArgs = {
        inherit src version;
        strictDeps = true;
      };

      # One dependency build, shared by every crate that follows. `drawbar` is left
      # out on purpose: it is the only crate carrying a GUI stack, and building
      # `nord-format` should not mean compiling eframe first.
      cargoArtifacts = craneLib.buildDepsOnly (
        commonArgs
        // {
          pname = "workspace";
          cargoExtraArgs = "--locked --workspace --exclude drawbar";
        }
      );

      # What a manifest cannot state about itself.
      extraArgs.nord-cli.meta.mainProgram = "nord";

      # Test-only Cargo features are declared by the crate itself, under
      # `[package.metadata.nix] testFeatures = [ … ]`, so adding a crate needs no
      # wiring here — only a `members` entry in the workspace manifest.
      mkCrate =
        member:
        let
          crate = (lib.importTOML (workspace + "/${member}/Cargo.toml")).package;
          testFeatures = crate.metadata.nix.testFeatures or [ ];
        in
        lib.nameValuePair crate.name (
          craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
              pname = crate.name;
              cargoExtraArgs = "--locked -p ${crate.name}";
              cargoTestExtraArgs = lib.optionalString (
                testFeatures != [ ]
              ) "--features ${lib.concatStringsSep "," testFeatures}";
            }
            // extraArgs.${crate.name} or { }
          )
        );

      crates = lib.listToAttrs (map mkCrate members);
    in
    {
      _module.args.rust = {
        inherit
          commonArgs
          craneLibFor
          edition
          version
          ;
        rustfmt = fenixPkgs.stable.rustfmt;
      };

      packages = crates;

      # A check per crate, minus `drawbar`: the GUI build has only ever been
      # exercised on darwin, and `nix flake check` on Linux is not where its link
      # dependencies should first be discovered. `nix build .#drawbar` builds it.
      checks = lib.mapAttrs' (name: lib.nameValuePair "${name}-native") (
        lib.removeAttrs crates [ "drawbar" ]
      );

      devShells.default = pkgs.mkShell {
        packages = [
          (fenixPkgs.combine [
            fenixPkgs.rust-analyzer
            fenixPkgs.stable.cargo
            fenixPkgs.stable.clippy
            fenixPkgs.stable.rust-src
            fenixPkgs.stable.rustc
            fenixPkgs.stable.rustfmt
            # `crates/.cargo/config.toml` targets wasm32 for the web build.
            fenixPkgs.targets.wasm32-unknown-unknown.stable.rust-std
          ])
        ];
      };
    };
}
