{
  binutils,
  callPackage,
  drawbar,
  guiLibs,
  lib,
  mesa,
  patchelf,
  repository,
  runCommand,
  squashfsTools,
  stdenv,
  stdenvNoCC,
  writeText,
  xvfb,
}:
let
  inherit (lib)
    concatMapStrings
    concatStringsSep
    makeLibraryPath
    optional
    unique
    ;

  runtime = callPackage ./runtime.nix { };

  # makeWrapper keeps the tested binary beside its wrapper.
  program = "${drawbar}/bin/.drawbar-wrapped";

  # What drawbar asks of the distribution it runs on. The install page states the
  # glibc release. The window system and OpenGL are loaded by name once it runs.
  host = {
    glibc = "2.39";
    libraries = [
      "libasound.so.2"
      "libc.so.6"
      "libgcc_s.so.1"
      "libm.so.6"
    ];
    loader =
      {
        aarch64-linux = "/lib/ld-linux-aarch64.so.1";
        x86_64-linux = "/lib64/ld-linux-x86-64.so.2";
      }
      .${stdenv.hostPlatform.system};
  };

  # The runtime is static, so what it links ships inside the AppImage.
  shipped = [ runtime ] ++ runtime.linked;

  # Sources whose license file names a text it keeps in a file named otherwise.
  noticeFiles.fuse = [ "LGPL2.txt" ];

  # A package's license files, verbatim, from the top two levels of its source.
  licenseFiles =
    pkg:
    runCommand "${pkg.name}-license-files" { inherit (pkg) src; } ''
      if [ -d "$src" ]; then
        cd "$src"
      else
        unpackPhase
        cd "$sourceRoot"
      fi
      find . -maxdepth 2 -type f \( -iname 'copying*' -o -iname 'licen[cs]e*' \
        -o -iname 'notice*' -o -iname 'copyright*' \) -exec install -Dm644 {} "$out/{}" \;
      for file in ${toString (noticeFiles.${pkg.pname} or [ ])}; do
        install -Dm644 "$file" "$out/$file"
      done
      if [ ! -d "$out" ]; then
        echo "${pkg.name}'s source holds no license file in its top two levels" >&2
        exit 1
      fi
    '';

  sourceLine =
    pkg:
    let
      urls = pkg.src.urls or (optional (pkg.src ? url) pkg.src.url);
    in
    "${pkg.pname}\t${pkg.version}\t${concatStringsSep " " urls}";

  sources = writeText "sources.tsv" (
    concatMapStrings (line: line + "\n") (
      [ "drawbar\t${drawbar.version}\t${repository}" ] ++ unique (map sourceLine shipped)
    )
  );
in
stdenvNoCC.mkDerivation {
  pname = "drawbar-appimage";
  inherit (drawbar) version;

  dontUnpack = true;

  nativeBuildInputs = [
    binutils
    patchelf
    squashfsTools
  ];

  buildPhase = ''
    runHook preBuild
    appdir=$PWD/AppDir
    licenses=$appdir/usr/share/licenses

    install -D ${program} "$appdir/usr/bin/drawbar"
    patchelf --set-interpreter ${host.loader} --remove-rpath "$appdir/usr/bin/drawbar"
    bash ${./host-needs.bash} "$appdir/usr/bin/drawbar" \
      ${host.loader} ${host.glibc} ${toString host.libraries}
    ln -s usr/bin/drawbar "$appdir/AppRun"

    install -Dm644 ${../../LICENSE} "$licenses/drawbar/LICENSE"
    ${concatMapStrings (pkg: ''
      cp -r ${licenseFiles pkg} "$licenses/${pkg.name}"
    '') shipped}
    while IFS=$'\t' read -r package release urls; do
      if [[ -z $urls || $urls == *mirror://* ]]; then
        echo "$package $release names no source URL a reader can open: $urls" >&2
        exit 1
      fi
    done <${sources}
    {
      echo "Where the source of each part of this AppImage comes from: name, version, URL."
      tr '\t' ' ' <${sources}
      echo
    } >"$licenses/SOURCES.txt"

    cp ${./drawbar.desktop} "$appdir/drawbar.desktop"
    cp ${../favicon.svg} "$appdir/drawbar.svg"
    ln -s drawbar.svg "$appdir/.DirIcon"

    mksquashfs "$appdir" squashfs -all-root -comp zstd -Xcompression-level 19 -b 1M -quiet
    cat ${runtime}/bin/appimage-runtime squashfs >drawbar.AppImage
    chmod +x drawbar.AppImage
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    install -D drawbar.AppImage "$out/drawbar-$version-${stdenv.hostPlatform.system}.AppImage"
    runHook postInstall
  '';

  # Stripping would cut the squashfs off the end of the runtime.
  dontFixup = true;

  doInstallCheck = true;
  nativeInstallCheckInputs = [ xvfb ];
  # The build sandbox is no such distribution, so the libraries are nixpkgs' here.
  installCheckPhase = ''
    runHook preInstallCheck
    __EGL_VENDOR_LIBRARY_FILENAMES=${mesa}/share/glvnd/egl_vendor.d/50_mesa.json \
      __GLX_VENDOR_LIBRARY_NAME=mesa \
      bash ${./check.bash} "$out/drawbar-$version-${stdenv.hostPlatform.system}.AppImage" \
      "$(patchelf --print-interpreter ${program})" --library-path \
      "$(patchelf --print-rpath ${program}):${makeLibraryPath (guiLibs ++ [ mesa ])}"
    runHook postInstallCheck
  '';

  meta = {
    description = "drawbar as one Linux executable";
    platforms = lib.platforms.linux;
  };
}
