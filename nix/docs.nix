# The user guide.
{
  lib,
  mdbook,
  mdbook-mermaid,
  stdenvNoCC,
  workspace,
}:
stdenvNoCC.mkDerivation {
  pname = "drawbar-docs";
  inherit (workspace.manifests.drawbar) version;

  # `docs/book` is mdBook's own output directory, ignored by git; keeping it out
  # of the source means a local `mdbook build` cannot change this derivation.
  src = lib.cleanSourceWith {
    name = "docs";
    src = lib.cleanSource ../docs;
    filter = path: type: !(type == "directory" && lib.hasSuffix "/book" path);
  };

  nativeBuildInputs = [
    mdbook
    mdbook-mermaid
  ];

  dontConfigure = true;
  dontInstall = true;

  buildPhase = ''
    runHook preBuild
    mdbook build --dest-dir "$out"

    # A book whose summary renders nothing still exits 0.
    if [ ! -f "$out/index.html" ]; then
      echo "mdbook rendered no index.html" >&2
      exit 1
    fi

    # mdBook copies `additional-js` and nothing beside it.
    cp LICENSE-mermaid.txt "$out/"
    runHook postBuild
  '';

  meta.description = "the drawbar user guide";
}
