# The Cargo workspace as its manifests state it. Nothing here builds.
{ lib }:
let
  inherit (lib)
    attrNames
    concatMap
    concatStringsSep
    importTOML
    listToAttrs
    nameValuePair
    optionalString
    ;

  root = ../crates;

  manifest = (importTOML (root + "/Cargo.toml")).workspace;

  members = map (
    member:
    let
      dir = root + "/${member}";
    in
    {
      inherit dir;
      package = (importTOML (dir + "/Cargo.toml")).package;
    }
  ) manifest.members;

  # Every member's `[package]` table, keyed by the name cargo knows it by.
  manifests = listToAttrs (map (member: nameValuePair member.package.name member.package) members);

  # Crates declare test-only features in `package.metadata.nix.testFeatures`, so new
  # workspace members need no wiring.
  testFeaturesFor = name: manifests.${name}.metadata.nix.testFeatures or [ ];
in
{
  inherit manifests root testFeaturesFor;

  # Every member's directory, keyed as `manifests` is.
  dirs = listToAttrs (map (member: nameValuePair member.package.name member.dir) members);
  inherit (manifest.package) edition repository;

  # The crates with a `corpus` feature.
  corpusCrates = [
    "nord-format"
    "nord-usb"
  ];

  featureArgs =
    features: optionalString (features != [ ]) "--features ${concatStringsSep "," features}";

  testFeatures = concatMap (name: map (feature: "${name}/${feature}") (testFeaturesFor name)) (
    attrNames manifests
  );
}
