# `nord` for this host. It also runs the binary it installed, as an install check.
{ mkCrate, nordInstallCheck }:
(mkCrate {
  crate = "nord-cli";
  meta.mainProgram = "nord";
}).overrideAttrs
  (nordInstallCheck {
    bin = "bin/nord";
  })
