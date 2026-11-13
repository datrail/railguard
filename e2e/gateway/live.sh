# shellcheck shell=bash
# Sourced by both live harnesses' session.sh, run in their Terraform root.

elapsed() { local s=$(($(date +%s) - $1)); echo "$((s / 60))m $((s % 60))s"; }

# Builds and pushes $1, the host's image, from crates/$2/Dockerfile, and both stubs on WireMock image $3.
build_images() {
  local host=$1 crate=$2 wiremock=$3 gateway repo registry
  gateway=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
  repo=$(cd "$gateway/../.." && pwd)
  registry=$(terraform output -raw registry)
  gcloud auth configure-docker "${registry%%/*}" --quiet >/dev/null
  # Cloud Run runs linux/amd64, whatever the machine building.
  docker build --platform linux/amd64 -t "$registry/$host:session" \
    -f "$repo/crates/$crate/Dockerfile" "$repo"
  docker build --platform linux/amd64 -t "$registry/rail-center:session" \
    -f "$gateway/stub.Dockerfile" --build-arg "WIREMOCK=$wiremock" "$gateway/rail-center-mappings"
  docker build --platform linux/amd64 -t "$registry/upstream:session" \
    -f "$gateway/stub.Dockerfile" --build-arg "WIREMOCK=$wiremock" "$gateway/upstream-mappings"
  for image in "$host" rail-center upstream; do
    docker push --quiet "$registry/$image:session"
  done
}

# The upstream stub answers a GET with 405 "stream not offered"; anything else
# is something still starting. The load balancer may still answer some errors
# after its first good answer, so it must give six in a row. $1 names what's
# waited on.
await_upstream() {
  local cert url body streak=0 deadline=$((SECONDS + 600))
  cert=$(mktemp)
  terraform output -raw lb_cert > "$cert"
  url=$(terraform output -raw mcp_url)
  while ((SECONDS < deadline)); do
    body=$(curl -s --max-time 10 --cacert "$cert" "$url" || true)
    if [[ "$body" == *"stream not offered"* ]]; then
      streak=$((streak + 1))
      if ((streak == 6)); then
        rm -f "$cert"
        return 0
      fi
    else
      streak=0
    fi
    sleep 5
  done
  rm -f "$cert"
  echo "$1 still doesn't reach the upstream stub after 10 minutes." >&2
  echo "Last answer:" >&2
  echo "$body" >&2
  return 1
}

# The stubs are open, so their journals and mappings must not be.
check_stub_admin() {
  local url code
  for url in "$(terraform output -raw rail_center_url)" "$(terraform output -raw upstream_url)"; do
    code=$(curl -s -o /dev/null -w '%{http_code}' "$url/__admin/requests")
    if [[ "$code" != 401 ]]; then
      echo "$url/__admin answers $code without the password; it must be 401." >&2
      return 1
    fi
  done
}
