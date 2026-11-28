# Attributes that run the CLI checks against an installed `nord`, through an
# emulator when it is foreign. This proves that the binary runs and behaves, not
# only that it links.
{ lib }:
let
  # The read-only inventory sweep, replayed. Exercises transport → wire → session
  # → op → CLI without a device, so the same proof runs on every target.
  script = ../nord-usb/tests/scripts/device/inventory.script;

  # An editor-written Sample Editor project, for the file-verb edit check.
  project = ../nord-format/tests/fixtures/nsmpproj/one-zone.nsmpproj;
in
{
  bin,
  emulator ? null,
}:
{
  doInstallCheck = true;
  installCheckPhase = ''
    runHook preInstallCheck
    NORD_RUNNER=${lib.optionalString (emulator != null) emulator.cmd} \
      POC_SCRIPT=${script} \
      POC_PROJECT=${project} \
      bash ${./checks}/check.sh "$out/${bin}"
    runHook postInstallCheck
  '';
  nativeInstallCheckInputs = lib.optional (emulator != null) emulator.package;
}
