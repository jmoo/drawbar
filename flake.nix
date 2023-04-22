{
  inputs = {
    nixpkgs.url = "http://nixos.org/channels/nixos-22.11/nixexprs.tar.xz";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { nixpkgs, flake-utils, rust-overlay, ... }@inputs:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; overlays = [ rust-overlay.overlay ]; };
        lib = pkgs.lib;
        project = import ./project.nix { inherit pkgs lib; };
      in {
        devShell = pkgs.mkShell {
          buildInputs = project.packages;
        };
      }
    );
}
