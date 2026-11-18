{
  alsa-lib,
  alsa-plugins,
  bzip2,
  callPackage,
  dbus,
  drawbar,
  elfutils,
  expat,
  flac,
  glibc,
  guiLibs,
  lame,
  lib,
  libdrm,
  libffi,
  libgbm,
  libGL,
  libmpg123,
  libogg,
  libopus,
  libpciaccess,
  libpulseaudio,
  libsndfile,
  libvorbis,
  libx11,
  libxau,
  libxcb,
  libxcursor,
  libxdmcp,
  libxext,
  libxfixes,
  libxi,
  libxkbcommon,
  libxml2,
  libxrandr,
  libxrender,
  libxshmfence,
  libxxf86vm,
  lm_sensors,
  mesa,
  path,
  patchelf,
  procps,
  python3,
  repository,
  runCommand,
  squashfsTools,
  stdenv,
  stdenvNoCC,
  systemdMinimal,
  wayland,
  writeText,
  xvfb,
  xz,
  zlib,
  zstd,
}:
let
  inherit (lib)
    concatMapStrings
    concatStringsSep
    elem
    getLib
    hasPrefix
    head
    optional
    optionalString
    removePrefix
    splitString
    tail
    unique
    ;

  runtime = callPackage ./runtime.nix { };

  # makeWrapper keeps the tested binary beside its wrapper.
  program = "${drawbar}/bin/.drawbar-wrapped";

  # Every store path a bundled library may come from. Each one's license files ship
  # in the AppImage, and a library from a path not listed here fails the build.
  bundled =
    map getLib [
      alsa-lib
      alsa-plugins
      bzip2
      dbus
      elfutils
      expat
      flac
      glibc
      lame
      libdrm
      libffi
      libgbm
      libGL
      libmpg123
      libogg
      libopus
      libpciaccess
      libpulseaudio
      libsndfile
      libvorbis
      libx11
      libxau
      libxcb
      libxcursor
      libxdmcp
      libxext
      libxfixes
      libxi
      libxkbcommon
      libxml2
      libxrandr
      libxrender
      libxshmfence
      libxxf86vm
      lm_sensors
      mesa
      mesa.llvmPackages.libllvm
      stdenv.cc.cc
      systemdMinimal
      wayland
      xz
      zlib
      zstd
    ]
    ++ [ stdenv.cc.cc.libgcc ];

  # Sources that keep their license text in a file named otherwise.
  noticeFiles.libglvnd = [ "README.md" ];

  # Sources whose notices are only in the comment at the top of each C file.
  headerNotices = [ "libdrm" ];

  # A package's license files, verbatim, from the top two levels of its source.
  licenseFiles =
    pkg:
    runCommand "${pkg.name}-license-files"
      {
        inherit (pkg) src;
        nativeBuildInputs = [ zstd ] ++ optional (elem pkg.pname headerNotices) python3;
      }
      ''
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
        ${optionalString (elem pkg.pname headerNotices) ''
          mkdir -p "$out"
          python3 ${./header-notices.py} . "$out/file-headers.txt"
        ''}
        if [ ! -d "$out" ]; then
          echo "${pkg.name}'s source holds no license file in its top two levels" >&2
          exit 1
        fi
      '';

  noticesOf = pkgs: concatMapStrings (pkg: ''["${pkg}"]="${licenseFiles pkg}" '') pkgs;

  mirrors = import "${path}/pkgs/build-support/fetchurl/mirrors.nix";

  # A `mirror://site/file` URL at the site's first mirror.
  resolve =
    url:
    let
      parts = splitString "/" (removePrefix "mirror://" url);
    in
    if hasPrefix "mirror://" url then
      head mirrors.${head parts} + concatStringsSep "/" (tail parts)
    else
      url;

  sourceLine =
    pkg:
    let
      urls = pkg.src.urls or (optional (pkg.src ? url) pkg.src.url);
    in
    "${pkg.pname}\t${pkg.version}\t${concatStringsSep " " (map resolve urls)}";

  sources = writeText "sources.tsv" (
    concatMapStrings (line: line + "\n") (
      [ "drawbar\t${drawbar.version}\t${repository}" ]
      ++ unique (map sourceLine (bundled ++ [ runtime ] ++ runtime.linked))
    )
  );

  # glvnd loads the vendor by this name, from the bundle's libraries.
  mesaVendor = writeText "50_mesa.json" (
    builtins.toJSON {
      file_format_version = "1.0.0";
      ICD.library_path = "libEGL_mesa.so.0";
    }
  );
in
stdenvNoCC.mkDerivation {
  pname = "drawbar-appimage";
  inherit (drawbar) version;

  dontUnpack = true;

  nativeBuildInputs = [
    patchelf
    squashfsTools
  ];

  buildPhase = ''
    runHook preBuild
    appdir=$PWD/AppDir
    licenses=$appdir/usr/share/licenses
    mkdir -p "$appdir/usr/share/glvnd/egl_vendor.d" "$appdir/usr/share/alsa" "$licenses"

    bash ${./appdir.bash} "$appdir" drawbar ${program} \
      ${concatMapStrings (pkg: "${getLib pkg}/lib/lib*.so.* ") guiLibs} \
      ${mesa}/lib/libEGL_mesa.so.0 ${mesa}/lib/libGLX_mesa.so.0 \
      ${alsa-plugins}/lib/alsa-lib/libasound_module_{ctl,pcm}_pulse.so \
      >sources.tsv

    declare -A bundled=(${noticesOf bundled})
    declare -A used=()
    while IFS=$'\t' read -r library path; do
      store=$(cut -d/ -f1-4 <<<"$path")
      if [[ ! -v bundled[$store] ]]; then
        echo "usr/lib/$library comes from $store, which crates/drawbar/appimage/default.nix does not list in bundled" >&2
        exit 1
      fi
      used[$store]=1
    done <sources.tsv
    for store in "''${!bundled[@]}"; do
      if [[ ! -v used[$store] ]]; then
        echo "bundled lists $store, which supplies no library" >&2
        exit 1
      fi
    done

    declare -A runtime=(${noticesOf ([ runtime ] ++ map getLib runtime.linked)})
    for store in "''${!bundled[@]}"; do
      cp -r "''${bundled[$store]}" "$licenses/''${store#/nix/store/*-}"
    done
    for store in "''${!runtime[@]}"; do
      cp -r "''${runtime[$store]}" "$licenses/''${store#/nix/store/*-}"
    done
    install -Dm644 ${../../LICENSE} "$licenses/drawbar/LICENSE"

    while IFS=$'\t' read -r package release urls; do
      if [[ -z $urls ]]; then
        echo "$package $release names no source URL in its src" >&2
        exit 1
      fi
    done <${sources}
    {
      echo "Where the source of each part of this AppImage comes from: name, version, URL."
      tr '\t' ' ' <${sources}
      echo
    } >"$licenses/SOURCES.txt"

    cp ${mesaVendor} "$appdir/usr/share/glvnd/egl_vendor.d/50_mesa.json"
    cp -r ${alsa-lib}/share/alsa/{alsa.conf,cards,ctl,pcm} "$appdir/usr/share/alsa/"
    cp ${alsa-plugins}/etc/alsa/conf.d/99-pulseaudio-default.conf.example \
      "$appdir/usr/share/alsa/99-pulseaudio-default.conf"

    substitute ${./AppRun} "$appdir/AppRun" \
      --subst-var-by interpreter "$(basename "$(patchelf --print-interpreter ${program})")"
    chmod +x "$appdir/AppRun"
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
  nativeInstallCheckInputs = [
    procps
    xvfb
  ];
  installCheckPhase = ''
    runHook preInstallCheck
    bash ${./check.bash} "$out/drawbar-$version-${stdenv.hostPlatform.system}.AppImage"
    runHook postInstallCheck
  '';

  meta = {
    description = "drawbar as one Linux executable";
    platforms = lib.platforms.linux;
  };
}
