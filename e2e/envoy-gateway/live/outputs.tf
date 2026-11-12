# What session.sh and the driver read with terraform output.

output "mcp_url" {
  value = "https://${google_compute_global_address.lb.address}/mcp"
}

# The load balancer's self-signed certificate, for the driver to trust.
output "lb_cert" {
  value = tls_self_signed_cert.lb.cert_pem
}

output "rail_center_url" {
  value = google_cloud_run_v2_service.stub["rail-center"].uri
}

output "upstream_url" {
  value = google_cloud_run_v2_service.stub["upstream"].uri
}

# Basic auth for both stubs' /__admin, as user e2e.
output "stub_admin_password" {
  value     = random_password.stub_admin.result
  sensitive = true
}

output "gateway_service" {
  value = google_cloud_run_v2_service.gateway.name
}

output "gateway_url" {
  value = google_cloud_run_v2_service.gateway.uri
}

output "registry" {
  value = "${var.region}-docker.pkg.dev/${var.project_id}/${google_artifact_registry_repository.images.repository_id}"
}
