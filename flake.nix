{
  description = "Rust tools for reading, editing, and moving programs on Clavia Nord instruments.";

  inputs = {
    crane.url = "github:ipetkov/crane";

    # Rust toolchains with per-target std libraries. nixpkgs' rustc ships only the
    # host target plus wasm32, which is not enough to cross-compile nord-usb to
    # Windows/Linux. fenix exposes packages per-system, so this stays scoped to the
    # Rust builds.
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-parts.url = "github:hercules-ci/flake-parts";

    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      # x86_64-darwin is omitted: current nixpkgs-unstable has dropped it.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      imports = [
        inputs.treefmt-nix.flakeModule
        # The toolchain, the native builds and the shell; then the cross targets and
        # the end-to-end checks built on top of them.
        ./rust.nix
        ./cross.nix
      ];

      # One formatter for the tree, wired into `nix fmt` and into `nix flake check`.
      perSystem =
        { rust, ... }:
        {
          treefmt = {
            programs.nixfmt.enable = true;
            programs.rustfmt = {
              # rustfmt has to be told the edition, and the workspace manifest is
              # the one place that already knows it.
              inherit (rust) edition;
              enable = true;
              package = rust.rustfmt;
            };
            programs.taplo = {
              enable = true;
              # An array the author broke across lines stays that way; collapsing the
              # workspace's member list to one line would make every addition to it a
              # whole-line diff.
              settings.formatting.array_auto_collapse = false;
            };
          };
        };
    };
}
