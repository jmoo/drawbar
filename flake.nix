{
  description = "Read, edit and move the sounds on your Nord keyboard: an app, a command, and Rust libraries.";

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
            # launches the site that `nix build .#site` produces.
            apps.drawbar-web = {
              meta.description = "serve the drawbar browser build with its guide and open it";
              program = pkgs.lib.getExe pkgs.nord.drawbar-web-launch;
              type = "app";
            };

            # Rust tests run in the package builds: `nix build .#nord.all`.
            checks = {
              bump =
                pkgs.runCommand "check-bump"
                  {
                    nativeBuildInputs = with pkgs; [
                      cargo
                      git
                      jq
                    ];
                  }
                  ''
                    bash ${./scripts}/check-bump.bash
                    touch "$out"
                  '';
              clippy = pkgs.nord.clippy;
            };

            devShells.default = pkgs.lib.crane.devShell {
              LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath pkgs.nord.guiLibs;
              RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
              inputsFrom = pkgs.lib.attrValues pkgs.nord.crates;
              # scripts/*.bash (see their `nix-deps` lines), plus `mdbook serve docs`.
              packages = with pkgs; [
                cargo-about
                curl
                gh
                jq
                mdbook
                rust-analyzer
              ];
            };

            legacyPackages = pkgs;

            packages = pkgs.nord.crates // pkgs.nord.crossPackages // { inherit (pkgs.nord) docs site; };

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
              # Follow `source`d files so lib.bash's definitions count.
              settings.formatter.shellcheck.options = [ "--external-sources" ];
            };
          };
      }
    );
}
