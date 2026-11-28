# A page per release file that redirects to it at the given crate versions.
{
  lib,
  symlinkJoin,
  writeTextDir,
}:
let
  inherit (lib)
    elemAt
    filter
    readFile
    replaceStrings
    splitString
    tail
    ;

  # The stable name under drawbar.app/download/ of each release file scripts/assets.tsv lists.
  names = {
    drawbar-app = "macos";
    drawbar-appimage = "linux";
    drawbar-windows = "windows";
    nord-cli-linux-aarch64-tarball = "nord-linux-aarch64";
    nord-cli-linux-x86_64-tarball = "nord-linux-x86_64";
    nord-cli-windows = "nord-windows";
  };
in
versions:
symlinkJoin {
  name = "drawbar-downloads";
  paths = map (
    line:
    let
      row = splitString "\t" line;
      crate = elemAt row 0;
      tag = "${crate}-v${versions.${crate}}";
      asset = replaceStrings [ "@version@" ] [ versions.${crate} ] (elemAt row 5);
      url = "https://github.com/jmoo/drawbar/releases/download/${tag}/${asset}";
    in
    writeTextDir "download/${names.${elemAt row 2}}/index.html" ''
      <!doctype html>
      <meta charset="utf-8">
      <meta http-equiv="refresh" content="0; url=${url}">
      <title>${asset}</title>
      <p><a href="${url}">Download ${asset}</a></p>
    ''
  ) (tail (filter (line: line != "") (splitString "\n" (readFile ../scripts/assets.tsv))));
}
