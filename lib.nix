{ lib }:
rec {
  # Build one crate from a Cargo workspace as its own package. Shares the
  # workspace source + lock (path deps resolve), but compiles/tests only `name`
  # via `cargo -p`. Test-only Cargo features are declared by the crate itself,
  # under `[package.metadata.nix] testFeatures = [ … ]`, so adding a crate
  # still needs no wiring here.
  mkRustCrate =
    pkgs: workspace: member: name:
    let
      testFeatures =
        (lib.importTOML (workspace + "/${member}/Cargo.toml")).package.metadata.nix.testFeatures or [ ];
    in
    pkgs.rustPlatform.buildRustPackage {
      pname = name;
      version = (lib.importTOML (workspace + "/Cargo.toml")).workspace.package.version;
      src = workspace;
      cargoLock.lockFile = workspace + "/Cargo.lock";
      cargoBuildFlags = [
        "-p"
        name
      ];
      cargoTestFlags = [
        "-p"
        name
      ]
      ++ lib.optionals (testFeatures != [ ]) [
        "--features"
        (lib.concatStringsSep "," testFeatures)
      ];
    };

  # Package every member of the `workspace` Cargo workspace, keyed by each
  # crate's real `package.name`. Add a crate to `members` in the workspace
  # Cargo.toml and it appears as `pkgs.<name>` with no further wiring.
  mkRustCrates =
    pkgs: workspace:
    let
      members = (lib.importTOML (workspace + "/Cargo.toml")).workspace.members;
      crateName = member: (lib.importTOML (workspace + "/${member}/Cargo.toml")).package.name;
    in
    lib.listToAttrs (
      map (
        member:
        let
          name = crateName member;
        in
        {
          inherit name;
          value = mkRustCrate pkgs workspace member name;
        }
      ) members
    );
}
