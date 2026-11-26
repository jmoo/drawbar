# What every cargo build of the workspace starts from.
{
  craneLib,
  lib,
  workspace,
}:
let
  inherit (lib) cleanSourceWith hasInfix hasSuffix;
  inherit (workspace) root;
in
{
  src = cleanSourceWith {
    name = "source";
    src = root;
    filter =
      path: type:
      (
        craneLib.filterCargoSources path type
        || hasSuffix ".script" path
        || hasInfix "/nord-cli/checks/" path
        # The About box's test compares its copy against this.
        || path == toString (root + "/LICENSE")
        # The committed specimens and replay scripts, whatever their extensions.
        || hasInfix "/tests/fixtures/" path
        || hasInfix "/tests/scripts/" path
        # The vendored glyphs and fonts, which drawbar `include_bytes!`s.
        || hasInfix "/drawbar/assets/" path
        # The license texts the About box `include_str!`s.
        || hasInfix "/drawbar/licenses/" path
        # The page, whose small-screen gate a test holds to the shell's.
        || path == toString (root + "/drawbar/index.html")
        # The page's icon, which a test holds to the titlebar's mark.
        || path == toString (root + "/drawbar/favicon.svg")
        # The collector's schema, which a test holds to the rows drawbar sends.
        || path == toString (root + "/drawbar/telemetry.json")
      );
  };

  strictDeps = true;
}
