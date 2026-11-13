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
#   down  deletes the traffic extensions, terraform destroy, then lists the
#         extensions left, which must be none.
set -euo pipefail
live=$(cd "$(dirname "$0")" && pwd)
stack=$(cd "$live/.." && pwd)
e2e=$(cd "$stack/.." && pwd)
cd "$live"
if [[ ! -f terraform.tfvars ]]; then
  echo "Copy terraform.tfvars.example to terraform.tfvars first." >&2
  exit 1
fi

# shellcheck source=../../gateway/live.sh
source "$e2e/gateway/live.sh"
project=$(sed -n 's/^project_id *= *"\(.*\)"/\1/p' terraform.tfvars)
prefix=$(sed -n 's/^prefix *= *"\(.*\)"/\1/p' terraform.tfvars)
# The images the local stack pins.
pinned() { sed -n "s#^ *image: \($1:.*\)\$#\1#p" "$stack/compose.yml"; }

# The session's traffic extension, by name; anchored, so prefix `rg` misses `rg-2`'s.
list_extensions() {
  gcloud service-extensions lb-traffic-extensions list --location=global \
    --project="$project" --filter="name~/${prefix}-ext\$" --format='value(name)'
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
  # Without it, the log query would fail quietly and the run would seem to log nothing.
  if [[ -z "$time" || ${#month} -ne 3 || "$months" != *"$month"* ]]; then
    echo "No Date header from www.googleapis.com" >&2
    return 1
  fi
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
    build_images gateway rail-envoy "$(pinned wiremock/wiremock)"
    terraform apply -auto-approve -input=false
    echo
    echo "== Waiting for the load balancer to reach the upstream stub"
    # The load balancer and the extension each take minutes to apply.
    await_upstream "The load balancer"
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
    # Deleted first, since Terraform skips a tainted extension's destroy step; a failed list finds none.
    for name in $(list_extensions || true); do
      gcloud service-extensions lb-traffic-extensions delete "$name" --location=global \
        --project="$project" --quiet
    done
    # The check below is for a failed destroy too.
    failed=0
    terraform destroy -auto-approve -input=false || failed=$?
    echo
    echo "Session down in $(elapsed "$start")."
    left=$(list_extensions)
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
