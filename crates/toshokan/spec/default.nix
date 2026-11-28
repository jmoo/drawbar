# TLC over every model of one suite. A model that violates a property must report
# that violation; any other must pass.
{
  lib,
  runCommand,
  suite,
  tlaplus18,
  toshokan-spec-configs,
}:
let
  inherit (lib)
    concatLines
    escapeShellArgs
    filterAttrs
    mapAttrsToList
    ;
  inherit (import ./models.nix) models;
in
runCommand suite
  {
    nativeBuildInputs = [ tlaplus18 ];
    src = toshokan-spec-configs;
  }
  ''
    cp "$src"/* .
    workers=''${NIX_BUILD_CORES:-0}
    [ "$workers" -gt 0 ] && [ "$workers" -le 8 ] || workers=8
    check_model() {
      status=0
      tlc -workers "$workers" -lncheck final -cleanup -config "$1.cfg" Portable.tla \
        > "$1.log" 2>&1 || status=$?
      if ! grep -q "$2" "$1.log"; then
        cat "$1.log" >&2
        echo "$1: expected \"$2\", TLC exited $status" >&2
        exit 1
      fi
      echo "$1: $2"
      grep -E 'distinct states found|depth of|Finished in' "$1.log" | tail -n 3
    }
    ${concatLines (
      mapAttrsToList (
        name: model:
        escapeShellArgs [
          "check_model"
          name
          (if model ? violates then "${model.violates} is violated" else "No error has been found")
        ]
      ) (filterAttrs (_: model: model.suite == suite) models)
    )}
    touch "$out"
  ''
