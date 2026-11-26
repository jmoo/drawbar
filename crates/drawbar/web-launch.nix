# Serve the site on loopback and open it, so the guide the app links to is there
# under /docs.
{
  miniserve,
  site,
  writeShellApplication,
}:
writeShellApplication {
  name = "drawbar-web";
  runtimeInputs = [ miniserve ];
  text = ''
    port="''${DRAWBAR_PORT:-8080}"
    url="http://127.0.0.1:$port/"

    # ⚠️ Store paths carry epoch mtimes, so without this the browser's heuristic
    # freshness keeps a stale bundle for years and the glue no longer matches
    # the wasm. `no-cache` forces revalidation; the ETag still answers 304.
    miniserve --index index.html --interfaces 127.0.0.1 --port "$port" \
      --header "Cache-Control: no-cache" \
      ${site} &
    server=$!
    trap 'kill "$server" 2>/dev/null || true' EXIT

    for _ in $(seq 40); do
      if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
        break
      fi
      sleep 0.25
    done

    if command -v open >/dev/null 2>&1; then
      open "$url"
    elif command -v xdg-open >/dev/null 2>&1; then
      xdg-open "$url"
    else
      echo "no opener found; open $url yourself" >&2
    fi

    wait "$server"
  '';
}
