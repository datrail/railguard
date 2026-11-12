# The global external Application Load Balancer in front of the upstream stub,
# its backends, and the traffic extension calling the gateway.

resource "google_compute_global_address" "lb" {
  name       = "${var.prefix}-lb-ip"
  ip_version = "IPV4"

  depends_on = [google_project_service.apis]
}

# A self-signed certificate that only needs to match the IP: the driver
# trusts it from an output. The private key stays in the local state, and
# nothing else trusts this certificate.
resource "tls_private_key" "lb" {
  algorithm = "RSA"
  rsa_bits  = 2048
}

resource "tls_self_signed_cert" "lb" {
  private_key_pem = tls_private_key.lb.private_key_pem

  subject {
    common_name = google_compute_global_address.lb.address
  }
  ip_addresses = [google_compute_global_address.lb.address]

  validity_period_hours = 8760 # 1 year
  allowed_uses          = ["digital_signature", "key_encipherment", "server_auth"]
}

# name_prefix and create_before_destroy: a certificate in use by the proxy
# can't be deleted, so a new one is created and swapped in first.
resource "google_compute_ssl_certificate" "lb" {
  name_prefix = "${var.prefix}-cert-"
  certificate = tls_self_signed_cert.lb.cert_pem
  private_key = tls_private_key.lb.private_key_pem

  lifecycle {
    create_before_destroy = true
  }
}

# no-gateway names a Cloud Run service that never exists: a gateway nothing
# can reach.
locals {
  negs = {
    upstream   = google_cloud_run_v2_service.stub["upstream"].name
    gateway    = google_cloud_run_v2_service.gateway.name
    no-gateway = "${var.prefix}-no-gateway"
  }
}

resource "google_compute_region_network_endpoint_group" "neg" {
  for_each = local.negs

  name                  = "${var.prefix}-${each.key}-neg"
  region                = var.region
  network_endpoint_type = "SERVERLESS"

  cloud_run {
    service = each.value
  }

  depends_on = [google_project_service.apis]
}

# The extension's backends speak gRPC, so HTTP/2; the upstream's, HTTPS.
resource "google_compute_backend_service" "backend" {
  for_each = local.negs

  name                  = "${var.prefix}-${each.key}-bs"
  load_balancing_scheme = "EXTERNAL_MANAGED"
  protocol              = each.key == "upstream" ? "HTTPS" : "HTTP2"

  backend {
    group = google_compute_region_network_endpoint_group.neg[each.key].id
  }

  log_config {
    enable      = true
    sample_rate = 1
  }
}

resource "google_compute_url_map" "lb" {
  name            = "${var.prefix}-um"
  default_service = google_compute_backend_service.backend["upstream"].id
}

resource "google_compute_target_https_proxy" "lb" {
  name             = "${var.prefix}-proxy"
  url_map          = google_compute_url_map.lb.id
  ssl_certificates = [google_compute_ssl_certificate.lb.id]
}

resource "google_compute_global_forwarding_rule" "lb" {
  name                  = "${var.prefix}-fr"
  load_balancing_scheme = "EXTERNAL_MANAGED"
  ip_address            = google_compute_global_address.lb.id
  target                = google_compute_target_https_proxy.lb.id
  port_range            = "443"
}
