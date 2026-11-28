# What a release attaches for one static target: `nord`, the project's license, and
# the notices of what is compiled into it.
{
  commonArgs,
  craneLib,
  cross,
  crossEnv,
  spec,
  target,
  workspace,
}:
let
  inherit (workspace.manifests.nord-cli) version;
  inherit (spec.crossPkgs.stdenv.hostPlatform) parsed;
  stem = "nord-${version}-${parsed.cpu.name}-${parsed.kernel.name}";
  notices = ./THIRD-PARTY-NOTICES.txt;
in
craneLib.mkCargoDerivation (
  commonArgs
  // {
    pname = "nord-cli-${target}-tarball";
    inherit version;
    cargoArtifacts = null;
    doInstallCargoArtifacts = false;
    # Every registry crate linked into the binary must be named in the notices.
    buildPhaseCargoCommand = ''
      cargo tree --locked --offline -p nord-cli --edges normal,no-proc-macro \
        --target ${crossEnv.tripleOf spec} --prefix none --format '{p}' |
        sed -e 's/ (\*)$//' | grep -v ' (/' | sed -e 's/ v/ /' | sort -u >linked
      [ -s linked ] || {
        echo "cargo tree listed no crates" >&2
        exit 1
      }
      while read -r crate; do
        grep -qxF -- "- $crate" ${notices} || unlisted="''${unlisted:+$unlisted, }$crate"
      done <linked
      if [ -n "''${unlisted-}" ]; then
        echo "THIRD-PARTY-NOTICES.txt lacks $unlisted; run scripts/licenses.bash" >&2
        exit 1
      fi
    '';
    installPhaseCommand = ''
      mkdir -p "${stem}" "$out"
      cp ${cross.nord-cli.${target}}/nord "${stem}/nord"
      cp ${../LICENSE} "${stem}/LICENSE"
      cp ${notices} "${stem}/THIRD-PARTY-NOTICES.txt"
      chmod -R u+w,go-w "${stem}"
      tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$SOURCE_DATE_EPOCH" -c "${stem}" |
        gzip -9n >"$out/${stem}.tar.gz"
    '';
  }
)
