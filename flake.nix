{
  description = "Rust tools for reading, editing, and moving programs on Clavia Nord instruments.";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

    flake-parts.url = "github:hercules-ci/flake-parts";

    # Rust toolchains with per-target std libraries. nixpkgs' rustc ships only the
    # host target plus wasm32, which is not enough to cross-compile nord-usb to
    # Windows/Linux. fenix exposes packages per-system, so this stays scoped to the
    # Rust builds.
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs@{ flake-parts, nixpkgs, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      # x86_64-darwin is omitted: current nixpkgs-unstable has dropped it.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      imports = [ ./modules/rust-cross.nix ];

      perSystem =
        { system, ... }:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ (import ./overlay.nix { inherit (nixpkgs) lib; }) ];
          };
        in
        {
          _module.args.pkgs = pkgs;

          packages = {
            inherit (pkgs)
              nord-bits-derive
              nord-cli
              nord-format
              nord-usb
              nord-web-demo
              ;
          };

          # nord-format-native and nord-usb-native are already declared by
          # modules/rust-cross.nix (imported above); avoid redefining them.
          checks = {
            nord-bits-derive-native = pkgs.nord-bits-derive;
            nord-cli-native = pkgs.nord-cli;
          };

          devShells.default = pkgs.mkShell {
            packages = [
              pkgs.cargo
              pkgs.rustc
              pkgs.rust-analyzer
              pkgs.clippy
              pkgs.rustfmt
            ];
          };
        };
    };
}
