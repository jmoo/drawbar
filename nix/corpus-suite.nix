# One crate's suite over a corpus assembly, with its `corpus` feature on.
{
  corpus,
  crate,
  mkCrate,
  tier ? "corpus",
  workspace,
}:
mkCrate {
  inherit crate;
  NORD_CORPUS_ROOT = "${corpus}";
  cargoTestExtraArgs = workspace.featureArgs (workspace.testFeaturesFor crate ++ [ "corpus" ]);
  pname = "${crate}-${tier}";
}
