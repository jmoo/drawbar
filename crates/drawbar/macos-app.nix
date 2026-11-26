# drawbar.app around the binary `packages` builds, so CI compiles drawbar once.
# scripts/dmg.bash checks it with the system's own tools and makes the disk image.
{
  crates,
  darwin,
  file,
  lib,
  libiconv,
  libicns,
  resvg,
  runCommand,
  workspace,
  writeText,
}:
let
  inherit (workspace.manifests.drawbar) version;
  bin = crates.drawbar.overrideAttrs { doCheck = false; };
  info = writeText "Info.plist" (
    lib.generators.toPlist { escape = true; } {
      CFBundleDisplayName = "drawbar";
      CFBundleExecutable = "drawbar";
      CFBundleIconFile = "drawbar";
      CFBundleIdentifier = "app.drawbar";
      CFBundleInfoDictionaryVersion = "6.0";
      CFBundleName = "drawbar";
      CFBundlePackageType = "APPL";
      CFBundleShortVersionString = version;
      CFBundleVersion = version;
      LSMinimumSystemVersion = "@minos@";
      NSHighResolutionCapable = true;
    }
  );
in
runCommand "drawbar-app-${version}"
  {
    nativeBuildInputs = [
      darwin.binutils-unwrapped
      file
      libicns
      resvg
    ];
    meta.description = "drawbar as a macOS application bundle";
  }
  ''
    contents="$out/drawbar.app/Contents"
    mkdir -p "$contents/MacOS" "$contents/Resources"
    install -m 755 ${bin}/bin/drawbar "$contents/MacOS/drawbar"

    # ⚠️ The Rust standard library links libiconv, and nixpkgs resolves it to its own
    # build. Nothing calls into it; the system's copy has the same compatibility
    # version. cctools' install_name_tool renews the linker's ad-hoc signature.
    install_name_tool -change ${libiconv}/lib/libiconv.2.dylib /usr/lib/libiconv.2.dylib \
      "$contents/MacOS/drawbar"

    # A library outside these two trees is missing from a Mac without Nix.
    while IFS= read -r -d "" file; do
      case "$(file -b "$file")" in Mach-O*) ;; *) continue ;; esac
      foreign="$(otool -L "$file" | tail -n +2 | awk '{ print $1 }' | grep -Ev '^(/usr/lib|/System/Library)/' || true)"
      if [ -n "$foreign" ]; then
        echo "''${file#"$out/"} links what a Mac without Nix lacks:" $foreign >&2
        exit 1
      fi
    done < <(find "$out" -type f -print0)

    minos="$(otool -l "$contents/MacOS/drawbar" | awk '$1 == "cmd" { cmd = $2 } cmd == "LC_BUILD_VERSION" && $1 == "minos" { print $2; exit }')"
    [ -n "$minos" ] || {
      echo "drawbar carries no LC_BUILD_VERSION to read its minimum macOS from" >&2
      exit 1
    }
    substitute ${info} "$contents/Info.plist" --replace-fail @minos@ "$minos"

    # The icon is the favicon, which is the titlebar's mark.
    for size in 16 32 128 256 512 1024; do
      resvg -w "$size" -h "$size" ${./favicon.svg} "$size.png"
    done
    png2icns "$contents/Resources/drawbar.icns" {16,32,128,256,512,1024}.png
  ''
