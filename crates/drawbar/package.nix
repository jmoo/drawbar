# drawbar for this host, wrapped on Linux so the display libraries it `dlopen`s are
# on the loader path.
{
  audioArgs,
  commonArgs,
  craneLib,
  guiLibs,
  lib,
  makeWrapper,
  mkCrate,
  workspace,
}:
let
  # The shared dependency build excludes drawbar, so its GUI stack needs one of its
  # own; without one, every edit under `drawbar/src` recompiles all of it.
  cargoArtifacts = craneLib.buildDepsOnly (
    commonArgs
    // audioArgs
    // {
      cargoExtraArgs = "--locked -p drawbar";
      pname = "drawbar";
      inherit (workspace.manifests.drawbar) version;
    }
  );

  pkg = mkCrate (
    {
      inherit cargoArtifacts;
      crate = "drawbar";
      meta.mainProgram = "drawbar";
    }
    // audioArgs
  );
in
if guiLibs == [ ] then
  pkg
else
  pkg.overrideAttrs (old: {
    nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ [ makeWrapper ];
    postInstall = ''
      wrapProgram "$out/bin/drawbar" \
        --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath guiLibs}
    '';
  })
