{ lib }:
let
  inherit (import ./lib.nix { inherit lib; }) mkRustCrates;
in
lib.composeExtensions (final: _: mkRustCrates final ./crates) (
  final: prev: {
    nord-cli = prev.lib.addMetaAttrs { mainProgram = "nord"; } prev.nord-cli;
  }
)
