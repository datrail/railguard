#!/usr/bin/env bash
# Starts and ends a paid session: rail-envoy's gateway on Cloud Run, called by
# GCP's Application Load Balancer through a traffic extension. Billed from
# `up` until `down`.
#
# Usage: ./session.sh up | test | down
#   up    builds and pushes the gateway's and the stubs' images, applies the
#         root, then waits until a call through the load balancer reaches the
#         upstream stub.
#   test  runs ../driver.py against the session, then prints the gateway's
#         warnings and errors from the run; the driver's exit code is the
#         result.
#   down  terraform destroy, then lists the traffic extensions left, which
#         must be none.
set -euo pipefail
live=$(cd "$(dirname "$0")" && pwd)
stack=$(cd "$live/.." && pwd)
e2e=$(cd "$stack/.." && pwd)
repo=$(cd "$e2e/.." && pwd)
cd "$live"
if [[ ! -f terraform.tfvars ]]; then
  echo "Copy terraform.tfvars.example to terraform.tfvars first." >&2
  exit 1
fi

elapsed() { local s=$(($(date +%s) - $1)); echo "$((s / 60))m $((s % 60))s"; }
project=$(sed -n 's/^project_id *= *"\(.*\)"/\1/p' terraform.tfvars)
prefix=$(sed -n 's/^prefix *= *"\(.*\)"/\1/p' terraform.tfvars)
# The images the local stack pins.
pinned() { sed -n "s#^ *image: \($1:.*\)\$#\1#p" "$stack/compose.yml"; }

build_images() {
  local registry wiremock
  registry=$(terraform output -raw registry)
  wiremock=$(pinned wiremock/wiremock)
  gcloud auth configure-docker "${registry%%/*}" --quiet >/dev/null
  # Cloud Run runs linux/amd64, whatever the machine building.
  docker build --platform linux/amd64 -t "$registry/gateway:session" \
    -f "$repo/crates/rail-envoy/Dockerfile" "$repo"
  docker build --platform linux/amd64 -t "$registry/rail-center:session" \
    -f "$e2e/gateway/stub.Dockerfile" --build-arg "WIREMOCK=$wiremock" "$e2e/gateway/rail-center-mappings"
  docker build --platform linux/amd64 -t "$registry/upstream:session" \
    -f "$e2e/gateway/stub.Dockerfile" --build-arg "WIREMOCK=$wiremock" "$e2e/gateway/upstream-mappings"
  for image in gateway rail-center upstream; do
    docker push --quiet "$registry/$image:session"
  done
}

# The load balancer and the extension each take minutes to apply. The upstream
# stub answers a GET with 405 "stream not offered", once the gateway has let
# it through; anything else is something still starting. The load balancer
# may still answer some errors after its first good answer, so it must give
# six in a row.
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
  echo "The load balancer still doesn't reach the upstream stub after 10 minutes." >&2
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

# What the gateway logged at WARNING or above since $1: a refused body mode
# names the mode the load balancer sent, or "unknown" if it sent none.
print_gateway_warnings() {
  local filter
  filter="resource.type=\"cloud_run_revision\""
  filter+=" AND resource.labels.service_name=\"$(terraform output -raw gateway_service)\""
  filter+=" AND severity>=WARNING AND timestamp>=\"$1\""
  gcloud logging read "$filter" --project="$project" --order=asc --limit=200 \
    --format='value(timestamp, severity, jsonPayload.message, jsonPayload.mode)'
}

# Now by Google's clock, not this machine's, which may be off by more than a run lasts.
read_google_time() {
  local _ day month year time months=JanFebMarAprMayJunJulAugSepOctNovDec before
  read -r _ day month year time _ < <(curl -sI https://www.googleapis.com/ | tr -d '\r' |
    sed -n 's/^[Dd]ate: //p')
  before=${months%%"$month"*}
  printf '%s-%02d-%sT%sZ' "$year" $((${#before} / 3 + 1)) "$day" "$time"
}

action=${1:-}
start=$(date +%s)
case "$action" in
  up)
    echo "Session start: $(date '+%F %T %Z'). It is billed until ./session.sh down."
    terraform init -input=false >/dev/null
    # The registry first: the services are deployed from images pushed to it.
    terraform apply -auto-approve -input=false \
      -target=google_artifact_registry_repository.images
    build_images
    terraform apply -auto-approve -input=false
    echo
    echo "== Waiting for the load balancer to reach the upstream stub"
    await_upstream
    check_stub_admin
    echo "Session up in $(elapsed "$start")."
    echo "END THE SESSION WITH ./session.sh down: it is billed while it exists."
    ;;
  test)
    since=$(read_google_time)
    # The image's user reads the certificate.
    cert=$(mktemp)
    chmod 644 "$cert"
    trap 'rm -f "$cert"' EXIT
    terraform output -raw lb_cert > "$cert"
    # Passed by name, so it stays off the command line.
    E2E_STUB_ADMIN_PASSWORD=$(terraform output -raw stub_admin_password)
    export E2E_STUB_ADMIN_PASSWORD
    result=0
    docker run --rm \
      -v "$stack/driver.py:/e2e/driver.py:ro" \
      -v "$e2e/gateway/gateway_cases.py:/e2e/gateway_cases.py:ro" -v "$cert:/e2e/lb-cert.pem:ro" \
      -e E2E_TARGET=live -e E2E_CA_FILE=/e2e/lb-cert.pem \
      -e E2E_MCP_URL="$(terraform output -raw mcp_url)" \
      -e E2E_UPSTREAM="$(terraform output -raw upstream_url)" \
      -e E2E_RAIL_CENTER="$(terraform output -raw rail_center_url)" \
      -e E2E_STUB_ADMIN_PASSWORD \
      "$(pinned python)" python -u /e2e/driver.py || result=$?
    echo
    echo "== The gateway's warnings and errors during the run"
    # A query sees a line only some time after it's logged; the streamed refusal is the run's last.
    warnings=""
    for _ in {1..8}; do
      sleep 15
      warnings=$(print_gateway_warnings "$since" || true)
      [[ "$warnings" == *"must send the body"* ]] && break
    done
    echo "${warnings:-none found}"
    exit "$result"
    ;;
  down)
    # The check below is for a failed destroy too.
    failed=0
    terraform destroy -auto-approve -input=false || failed=$?
    echo
    echo "Session down in $(elapsed "$start")."
    left=$(gcloud service-extensions lb-traffic-extensions list --location=global \
      --project="$project" --filter="name~/${prefix}-" --format='value(name)')
    echo "Traffic extensions left: ${left:-none}"
    if [[ -n "$left" ]]; then
      echo "WARNING: an extension left holds the forwarding rule; delete it, then down again." >&2
      exit 1
    fi
    exit "$failed"
    ;;
  *)
    sed -n '6,14p' "$0" >&2
    exit 1
    ;;
esac
