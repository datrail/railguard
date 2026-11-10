#!/usr/bin/env bash
# Starts and ends a paid Apigee session (the apigee-session root). The
# environment is billed from `up` until `down`.
#
# Usage: ./session.sh up | test | down
#   up    builds and pushes the callout's and the stubs' images, applies the
#         root, then waits until a call through the enforce proxy reaches the
#         upstream stub.
#   test  runs ../driver.py against the session, in the stand-in's Python image; its exit code
#         is the result.
#   down  terraform destroy, then lists the org's environments, which must be [].
# ../apigee-org must be applied first, once (README.md).
set -euo pipefail
live=$(cd "$(dirname "$0")" && pwd)
e2e=$(cd "$live/.." && pwd)
repo=$(cd "$e2e/../.." && pwd)
cd "$live/apigee-session"
[[ -f terraform.tfvars ]] || { echo "Copy apigee-session/terraform.tfvars.example to terraform.tfvars first." >&2; exit 1; }

api="https://apigee.googleapis.com/v1"
get() { curl -fsS -H "Authorization: Bearer $(gcloud auth print-access-token)" "${api}/$1"; }
elapsed() { local s=$(($(date +%s) - $1)); echo "$((s / 60))m $((s % 60))s"; }
# The org is named after the project.
org="organizations/$(sed -n 's/^project_id *= *"\(.*\)"/\1/p' terraform.tfvars)"

build_images() {
  local registry wiremock
  registry=$(terraform output -raw registry)
  wiremock=$(sed -n 's/^ *image: \(wiremock\/wiremock:.*\)$/\1/p' "$e2e/compose.yml")
  gcloud auth configure-docker "${registry%%/*}" --quiet >/dev/null
  # Cloud Run runs linux/amd64, whatever the machine building.
  docker build --platform linux/amd64 -t "$registry/callout:session" \
    -f "$repo/crates/rail-apigee/Dockerfile" "$repo"
  docker build --platform linux/amd64 -t "$registry/rail-center:session" \
    -f "$live/stub.Dockerfile" --build-arg "WIREMOCK=$wiremock" "$e2e/rail-center-mappings"
  docker build --platform linux/amd64 -t "$registry/upstream:session" \
    -f "$live/stub.Dockerfile" --build-arg "WIREMOCK=$wiremock" "$e2e/upstream-mappings"
  for image in callout rail-center upstream; do
    docker push --quiet "$registry/$image:session"
  done
}

# The load balancer, the environment's attachment and new IAM grants each take
# minutes to apply. The upstream stub answers a GET with 405 "stream not
# offered"; anything else is something still starting.
await_proxy() {
  local cert url body
  cert=$(mktemp)
  terraform output -raw lb_cert > "$cert"
  url=$(terraform output -raw enforce_url)
  for _ in $(seq 60); do
    body=$(curl -s --cacert "$cert" "$url" || true)
    if [[ "$body" == *"stream not offered"* ]]; then
      rm -f "$cert"
      return 0
    fi
    sleep 10
  done
  rm -f "$cert"
  echo "The enforce proxy still doesn't reach the upstream stub after 10 minutes. Last answer:" >&2
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

action=${1:-}
start=$(date +%s)
case "$action" in
  up)
    echo "Session start: $(date '+%F %T %Z'). The environment is billed until ./session.sh down."
    terraform init -input=false >/dev/null
    # The registry first: the services are deployed from images pushed to it.
    terraform apply -auto-approve -input=false \
      -target=google_artifact_registry_repository.images
    build_images
    terraform apply -auto-approve -input=false
    echo
    echo "== Deployments"
    # Base environments only take Standard policies.
    for deployment in $(terraform output -json deployments | jq -r '.[]'); do
      get "$deployment" | jq -c '{revision, state, proxyDeploymentType, serviceAccount}'
    done
    echo
    echo "== Waiting for the enforce proxy to reach the upstream stub"
    await_proxy
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
      -v "$e2e/driver.py:/e2e/driver.py:ro" -v "$cert:/e2e/lb-cert.pem:ro" \
      -e E2E_TARGET=live -e E2E_CA_FILE=/e2e/lb-cert.pem \
      -e E2E_ENFORCE_URL="$(terraform output -raw enforce_url)" \
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
    sed -n '5,10p' "$0" >&2
    exit 1
    ;;
esac
