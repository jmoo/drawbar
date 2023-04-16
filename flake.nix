{
  inputs = {
    nixpkgs.url = "http://nixos.org/channels/nixos-22.11/nixexprs.tar.xz";

    flake-utils.url = "github:numtide/flake-utils";

    home-manager.url = "github:nix-community/home-manager";
    home-manager.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { nixpkgs, flake-utils, home-manager, ... }@inputs:
    let
      lib = (import ./lib.nix) inputs;

      project = (lib.mkFlake {
        inherit inputs;
        imports = [ ./project.nix ];
      });

    in (removeAttrs project [ "lib" ]) // { lib = project.lib // lib; };
}
