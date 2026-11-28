# The GitHub Pages tree: the browser build at the root, the guide under /docs,
# and the download redirects under /download.
# Copies, not links: the tree leaves the store as a tarball.
# `web` and `nordCliVersion` are parameters so the deployed tree can pair released
# builds with this checkout's guide; scripts/site.bash overrides them, and passes
# the release before `web` as `previous`.
{
  docs,
  lib,
  mkDownloads,
  nordCliVersion ? workspace.manifests.nord-cli.version,
  previous ? null,
  runCommand,
  web,
  workspace,
}:
runCommand "drawbar-site-${web.version}"
  {
    meta.description = "drawbar and its guide, laid out for GitHub Pages";
  }
  ''
    mkdir -p "$out/docs"
    cp -rL ${web}/. "$out/"
    cp -rL ${docs}/. "$out/docs/"
    cp -rL ${
      mkDownloads {
        drawbar = web.version;
        nord-cli = nordCliVersion;
      }
    }/. "$out/"

    # Pages runs Jekyll over an unmarked tree and drops `_`-prefixed paths.
    touch "$out/.nojekyll"

    # The page's metadata names files by absolute URL, some of them the guide's.
    for url in $(grep -o 'https://drawbar\.app/[^"]*' "$out/index.html" | sort -u); do
      [ -e "$out/''${url#https://drawbar.app/}" ] || {
        echo "index.html names $url, which the site lacks" >&2
        exit 1
      }
    done

    ${lib.optionalString (previous != null) ''
      # A page cached before a deploy names the modules of the release before,
      # which stay beside the new ones under their own hashed names.
      chmod -R u+w "$out/pkg"
      cp -rL --update=none ${previous}/pkg/. "$out/pkg/"
    ''}
  ''
