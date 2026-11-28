# What native drawbar needs from the host beyond Rust, empty off Linux.
{
  alsa-lib,
  lib,
  libGL,
  libx11,
  libxcursor,
  libxi,
  libxkbcommon,
  libxrandr,
  pkg-config,
  stdenv,
  wayland,
}:
let
  inherit (stdenv.hostPlatform) isLinux;
in
{
  audioArgs = lib.optionalAttrs isLinux {
    buildInputs = [ alsa-lib ];
    nativeBuildInputs = [ pkg-config ];
  };

  # ⚠️ winit, glutin and xkbcommon `dlopen` these: builds succeed without them, then
  # die with `NoWaylandLib`. Packages and the dev shell must set the loader path.
  guiLibs = lib.optionals isLinux [
    libGL
    libx11
    libxcursor
    libxi
    libxkbcommon
    libxrandr
    wayland
  ];
}
