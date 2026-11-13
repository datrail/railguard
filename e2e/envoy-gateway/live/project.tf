# The APIs and identities a session needs.

resource "google_project_service" "apis" {
  for_each = toset([
    "compute.googleapis.com",
    "run.googleapis.com",
    "artifactregistry.googleapis.com",
    "iam.googleapis.com",
    "networkservices.googleapis.com",
  ])

  service = each.key
  # Left enabled on destroy: other work in the project may use them.
  disable_on_destroy = false
}

# The services' runtimes, with no roles.
resource "google_service_account" "sa" {
  for_each = toset(["gateway", "stub"])

  account_id   = "${var.prefix}-${each.key}-sa"
  display_name = "${var.prefix} ${each.key}"

  depends_on = [google_project_service.apis]
}

# The session's images; session.sh pushes them before the services are
# applied.
resource "google_artifact_registry_repository" "images" {
  repository_id = "${var.prefix}-images"
  location      = var.region
  format        = "DOCKER"

  depends_on = [google_project_service.apis]
}
