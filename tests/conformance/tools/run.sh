#!/usr/bin/env bash
# Run against an explicitly provisioned Marsa VM and writable shared directory.
set -euo pipefail
: "${DOCKER_HOST:?set DOCKER_HOST to the disposable Marsa unix socket}"
: "${DOCKER_CONFIG:?set DOCKER_CONFIG to a disposable Docker config directory}"
: "${MARSA_CONFORMANCE_SHARE:?set this to the writable shared host directory}"
[[ "$DOCKER_HOST" == unix:///* && -S "${DOCKER_HOST#unix://}" ]]
unset DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH DOCKER_AUTH_CONFIG
export TESTCONTAINERS_DOCKER_SOCKET_OVERRIDE=/var/run/docker.sock
export TESTCONTAINERS_HOST_OVERRIDE=127.0.0.1
export TESTCONTAINERS_RYUK_DISABLED=false
export MARSA_SAMPLE_IMAGE=busybox@sha256:a7dbc319b1438115a207e11930dc4bb3e643dcb3ec0bdd694bb36cd8fad289ec
docker_cli=${MARSA_TEST_DOCKER:-docker}
source_dir=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/marsa-tools.XXXXXXXX")
workspace=
label="dev.marsa.conformance=$(basename "$work")"
cleanup() {
  if [[ -n "$workspace" ]]; then
    while IFS= read -r id; do
      [[ -z "$id" ]] || "$docker_cli" rm -f "$id" >/dev/null
    done < <("$docker_cli" ps -aq --filter "label=$label")
    rm -rf "$workspace"
  fi
  rm -rf "$work"
}
trap cleanup EXIT
cp -R "$source_dir"/{java,go,node,python} "$work/"
node_installed=false
install_node() {
  if [[ "$node_installed" == false ]]; then
    (cd "$work/node" && npm ci --ignore-scripts --no-audit --no-fund)
    node_installed=true
  fi
}
if [[ $# -eq 0 ]]; then set -- java node go python devcontainer; fi
for sample in "$@"; do
  case "$sample" in
    java)
      mvn -B -ntp -f "$work/java/pom.xml" compile exec:exec -Dexec.executable=java "-Dexec.args=-classpath %classpath Smoke"
      ;;
    go)
      (cd "$work/go" && go test -mod=readonly -count=1 -v -timeout 4m ./...)
      ;;
    node)
      install_node
      node "$work/node/smoke.js"
      ;;
    python)
      python3 -m venv "$work/venv"
      "$work/venv/bin/python" -m pip install --disable-pip-version-check -r "$work/python/requirements.txt"
      "$work/venv/bin/python" "$work/python/smoke.py"
      ;;
    devcontainer)
      install_node
      workspace=$(mktemp -d "$MARSA_CONFORMANCE_SHARE/devcontainer.XXXXXXXX")
      cp -R "$source_dir/devcontainer/." "$workspace/"
      cli="$work/node/node_modules/.bin/devcontainer"
      "$cli" build --docker-path "$docker_cli" --workspace-folder "$workspace"
      "$cli" up --docker-path "$docker_cli" --workspace-folder "$workspace" --id-label "$label"
      "$cli" exec --docker-path "$docker_cli" --workspace-folder "$workspace" --id-label "$label" \
        sh -ec 'test "$(cat /image-marker)" = marsa-build && test "$(cat workspace-marker)" = marsa-workspace'
      ;;
    *)
      echo "Unknown conformance sample: $sample" >&2
      exit 2
      ;;
  esac
  echo "PASS $sample"
done
