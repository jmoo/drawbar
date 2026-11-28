# Portable.tla beside every model's TLC config.
{
  lib,
  linkFarm,
  writeText,
}:
let
  inherit (lib)
    all
    attrValues
    boolToString
    concatLines
    elem
    filter
    isBool
    mapAttrs
    mapAttrsToList
    optional
    optionals
    toUpper
    ;
  inherit (import ./models.nix) baseline checks models;

  # A model's config: the baseline's constants under its own, and its checks, or
  # only the property it violates.
  config =
    model:
    let
      checked =
        if model ? violates then
          mapAttrs (_: filter (name: name == model.violates)) checks.safety
        else
          checks.${model.checks};
      section = keyword: lines: optionals (lines != [ ]) ([ keyword ] ++ map (line: "  ${line}") lines);
      tla = value: if isBool value then toUpper (boolToString value) else toString value;
    in
    concatLines (
      [ "SPECIFICATION Spec" ]
      ++ section "CONSTANTS" (
        mapAttrsToList (name: value: "${name} = ${tla value}") (baseline // model.constants)
      )
      ++ section "INVARIANTS" checked.invariants
      ++ section "PROPERTIES" checked.properties
      ++ optional (model.symmetry or false) "SYMMETRY Symmetric"
      ++ [ "CHECK_DEADLOCK FALSE" ]
    );
in
assert all (
  model:
  elem model.suite [
    null
    "toshokan-spec"
    "toshokan-spec-deep"
  ]
) (attrValues models);
linkFarm "toshokan-spec-configs" (
  [
    {
      name = "Portable.tla";
      path = ./Portable.tla;
    }
  ]
  ++ mapAttrsToList (name: model: {
    name = "${name}.cfg";
    path = writeText "${name}.cfg" (config model);
  }) models
)
