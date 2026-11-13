#!/usr/bin/env bash
# Starts and ends a paid Apigee session (the apigee-session root). The
# environment is billed from `up` until `down`.
#
# Usage: ./session.sh up | test | down
#   up    builds and pushes the callout's and the stubs' images, applies the
#         root, then waits until a call through the mcp proxy reaches the
#         upstream stub.
#   test  runs ../driver.py against the session, in the stand-in's Python
#         image; its exit code is the result.
#   down  terraform destroy, then lists the org's environments, which must
#         be [].
# An Apigee organisation must exist first: README.md, "One-time setup".
set -euo pipefail
live=$(cd "$(dirname "$0")" && pwd)
e2e=$(cd "$live/.." && pwd)
cd "$live/apigee-session"
if [[ ! -f terraform.tfvars ]]; then
  echo "Copy apigee-session/terraform.tfvars.example to terraform.tfvars first." >&2
  exit 1
fi

api="https://apigee.googleapis.com/v1"
get() { curl -fsS -H "Authorization: Bearer $(gcloud auth print-access-token)" "${api}/$1"; }
# shellcheck source=../../gateway/live.sh
source "$e2e/../gateway/live.sh"
# The org is named after the project.
org="organizations/$(sed -n 's/^project_id *= *"\(.*\)"/\1/p' terraform.tfvars)"

action=${1:-}
start=$(date +%s)
case "$action" in
  up)
    echo "Session start: $(date '+%F %T %Z'). The environment is billed until ./session.sh down."
    terraform init -input=false >/dev/null
    # The registry first: the services are deployed from images pushed to it.
    terraform apply -auto-approve -input=false \
      -target=google_artifact_registry_repository.images
    wiremock=$(sed -n 's/^ *image: \(wiremock\/wiremock:.*\)$/\1/p' "$e2e/compose.yml")
    build_images callout rail-apigee "$wiremock"
    terraform apply -auto-approve -input=false
    echo
    echo "== Deployments"
    # Base environments only take Standard policies.
    for deployment in $(terraform output -json deployments | jq -r '.[]'); do
      get "$deployment" | jq -c '{revision, state, proxyDeploymentType, serviceAccount}'
    done
    echo
    echo "== Waiting for the mcp proxy to reach the upstream stub"
    # The load balancer, the environment's attachment and new IAM grants each take minutes to apply.
    await_upstream "The mcp proxy"
    check_stub_admin
    echo "Session up in $(elapsed "$start")."
    echo "END THE SESSION WITH ./session.sh down: the environment is billed while it exists."
    ;;
  test)
    # The image's user reads the certificate.
    cert=$(mktemp)
    chmod 644 "$cert"
    trap 'rm -f "$cert"' EXIT
    terraform output -raw lb_cert > "$cert"
    # Passed by name, so it stays off the command line.
    E2E_STUB_ADMIN_PASSWORD=$(terraform output -raw stub_admin_password)
    export E2E_STUB_ADMIN_PASSWORD
    docker compose -f "$e2e/compose.yml" build driver
    docker run --rm \
      -v "$e2e/driver.py:/e2e/driver.py:ro" \
      -v "$e2e/../gateway/gateway_cases.py:/e2e/gateway_cases.py:ro" -v "$cert:/e2e/lb-cert.pem:ro" \
      -e E2E_TARGET=live -e E2E_CA_FILE=/e2e/lb-cert.pem \
      -e E2E_MCP_URL="$(terraform output -raw mcp_url)" \
      -e E2E_NO_CALLOUT_URL="$(terraform output -raw no_callout_url)" \
      -e E2E_UPSTREAM="$(terraform output -raw upstream_url)" \
      -e E2E_RAIL_CENTER="$(terraform output -raw rail_center_url)" \
      -e E2E_STUB_ADMIN_PASSWORD \
      railguard-e2e-apigee-python python -u /e2e/driver.py
    ;;
  down)
    terraform destroy -auto-approve -input=false
    echo
    echo "Session down in $(elapsed "$start")."
    envs=$(get "${org}/environments" | jq -c '.')
    echo "Environments left in the org: ${envs}"
    if [[ "$envs" != "[]" ]]; then
      echo "WARNING: environments are billed while they exist." >&2
      exit 1
    fi
    ;;
  *)
    sed -n '5,12p' "$0" >&2
    exit 1
    ;;
esac
