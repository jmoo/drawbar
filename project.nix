{ config, pkgs, lib, ... }:
let
  rust-bin = pkgs.rust-bin.nightly.latest.default;

  rustPlatform = pkgs.makeRustPlatform {
    cargo = rust-bin;
    rustc = rust-bin;
  };
in
{
  name = "nord-utils";

  packages = with pkgs; [
    trunk
    rust-bin

    # Darwin only
    libiconv
    darwin.apple_sdk.frameworks.OpenGL
    darwin.apple_sdk.frameworks.CoreServices
    darwin.apple_sdk.frameworks.AppKit
    darwin.apple_sdk.frameworks.Foundation
    darwin.apple_sdk.frameworks.ApplicationServices
    darwin.apple_sdk.frameworks.CoreGraphics
    darwin.apple_sdk.frameworks.CoreVideo
  ];
}
