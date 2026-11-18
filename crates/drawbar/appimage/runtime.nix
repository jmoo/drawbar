# The AppImage type 2 runtime: a static executable that mounts the squashfs appended
# to it, or extracts it, and runs its AppRun. Built from source the way upstream's
# build script builds it, with nixpkgs' static toolchain.
{
  fetchFromGitHub,
  fetchurl,
  lib,
  pkgsStatic,
}:
let
  inherit (lib) filter hasSuffix;

  inherit (pkgsStatic)
    mimalloc
    stdenv
    zlib
    zstd
    ;

  rev = "fedfad51acc2860b2916a1eef32ba4a500ff31ef";

  runtimeSrc = fetchFromGitHub {
    owner = "AppImage";
    repo = "type2-runtime";
    inherit rev;
    hash = "sha256-OSwpZKe+r67kXGaSlYsIYRljmjnsxd7fHul62GwMUrA=";
  };

  # The runtime finds `fusermount` or `fusermount3` on the PATH and passes it in
  # `FUSERMOUNT_PROG`, which upstream's patch makes libfuse use, so either FUSE works.
  # The patch is written against the libfuse release upstream builds.
  fuse3 = pkgsStatic.fuse3.overrideAttrs (old: rec {
    version = "3.15.0";
    src = fetchurl {
      url = "https://github.com/libfuse/libfuse/releases/download/fuse-${version}/fuse-${version}.tar.xz";
      hash = "sha256-cFic/V4c/3zNaskchsAb40CyJyhcXiALqihOQB7qLKA=";
    };
    # nixpkgs' FUSERMOUNT_DIR patch is moot under upstream's, and does not apply to 3.15.
    patches =
      filter (patch: !hasSuffix "Do-not-set-FUSERMOUNT_DIR.patch" (toString patch)) old.patches
      ++ [ "${runtimeSrc}/patches/libfuse/mount.c.diff" ];
  });

  # runtime.c includes squashfuse's private headers, which it does not install.
  squashfuse = (pkgsStatic.squashfuse.override { inherit fuse3; }).overrideAttrs (old: {
    # The compressions mksquashfs writes here, and those upstream's runtime reads.
    configureFlags = old.configureFlags ++ [
      "--without-lz4"
      "--without-lzo"
      "--without-xz"
    ];
    postInstall = ''
      cp ./*.h "$out/include/squashfuse/"
    '';
  });
in
stdenv.mkDerivation {
  pname = "appimage-runtime";
  version = "0-unstable-${builtins.substring 0 7 rev}";
  src = runtimeSrc;

  buildInputs = [
    fuse3
    mimalloc
    squashfuse
    zlib
    zstd
  ];

  buildPhase = ''
    runHook preBuild
    cd src/runtime
    $CC -std=gnu99 -Os -D_FILE_OFFSET_BITS=64 -DGIT_COMMIT='"${builtins.substring 0 7 rev}"' \
      -I${fuse3.dev}/include/fuse3 -T data_sections.ld \
      -ffunction-sections -fdata-sections -Wl,--gc-sections -static -Wall -Werror \
      runtime.c -lsquashfuse -lsquashfuse_ll -lzstd -lz -lfuse3 -lmimalloc -o runtime
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    $STRIP --strip-debug --strip-unneeded runtime
    # The type 2 magic, which the linker script cannot place: "AI" and 2 at offset 8.
    printf 'AI\002' | dd of=runtime bs=1 count=3 seek=8 conv=notrunc status=none
    install -D runtime "$out/bin/appimage-runtime"
    runHook postInstall
  '';

  dontFixup = true;

  # What the static executable links, for its license notices.
  passthru.linked = [
    fuse3
    mimalloc
    squashfuse
    stdenv.cc.libc
    zlib
    zstd
  ];
}
