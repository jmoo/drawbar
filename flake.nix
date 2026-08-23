{
  description = "Rust tools for reading, editing, and moving programs on Clavia Nord instruments.";

  inputs = {
    crane.url = "github:ipetkov/crane";

    flake-parts.url = "github:hercules-ci/flake-parts";

    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs@{ flake-parts, nixpkgs, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } (
      { config, lib, ... }:
      {
        imports = [ inputs.treefmt-nix.flakeModule ];

        systems = [
          "x86_64-linux"
          "aarch64-linux"
          "aarch64-darwin"
        ];

        flake.overlays.default = lib.composeManyExtensions [
          (final: prev: { lib = prev.lib.extend (_: _: { crane = inputs.crane.mkLib final; }); })
          (import ./overlay.nix)
        ];

        perSystem =
          { system, pkgs, ... }:
          {
            _module.args.pkgs = import nixpkgs {
              inherit system;
              overlays = [ config.flake.overlays.default ];
            };

            # `nix run` prefers apps over packages, so `nix run .#drawbar-web`
            # launches the bundle that `nix build .#drawbar-web` produces.
            apps.drawbar-web = {
              meta.description = "serve the drawbar browser build and open it";
              program = pkgs.lib.getExe pkgs.nord.drawbar-web-launch;
              type = "app";
            };

            devShells.default = pkgs.lib.crane.devShell {
              inputsFrom = pkgs.lib.attrValues pkgs.nord.crates;
              # scripts/*.bash (see their `nix-deps` lines)
              packages = with pkgs; [
                gh
                jq
                rust-analyzer
              ];
              LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath pkgs.nord.guiLibs;
              RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
            };

            # `nix flake check`: formatting (treefmt, below) and clippy. Tests run inside
            # the package builds — `nix build .#nord.all` is the other half of CI.
            checks.clippy = pkgs.nord.clippy;

            legacyPackages = pkgs;

            packages = pkgs.nord.crates // pkgs.nord.crossPackages;

            treefmt = {
              programs = {
                nixfmt.enable = true;
                rustfmt = {
                  inherit (pkgs.nord) edition;
                  enable = true;
                  package = pkgs.nord.rustfmt;
                };
                shellcheck.enable = true;
                shfmt.enable = true;
                taplo = {
                  enable = true;
                  settings.formatting.array_auto_collapse = false;
                };
              };
            };
          };
      }
    );
}
