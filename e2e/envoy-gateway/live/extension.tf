# The provider can't set the body mode, REQUEST_TRAILERS or a 6 s timeout, so gcloud imports the extension.

locals {
  # The first chain whose condition matches runs; the driver picks one by header.
  chains = [
    {
      name    = "streamed"
      match   = "'x-e2e-chain' in request.headers && request.headers['x-e2e-chain'] == 'streamed'"
      backend = "gateway"
      mode    = "BODY_SEND_MODE_STREAMED"
    },
    {
      name    = "no-gateway"
      match   = "'x-e2e-chain' in request.headers && request.headers['x-e2e-chain'] == 'no-gateway'"
      backend = "no-gateway"
      mode    = "BODY_SEND_MODE_FULL_DUPLEX_STREAMED"
    },
    {
      name = "judged"
      # Only the driver's path, so a scanner's call is never judged and reported as a denial.
      match   = "request.path.startsWith('/mcp')"
      backend = "gateway"
      mode    = "BODY_SEND_MODE_FULL_DUPLEX_STREAMED"
    },
  ]

  extension_name = "projects/${var.project_id}/locations/global/lbTrafficExtensions/${var.prefix}-ext"
  extension = {
    name                = local.extension_name
    loadBalancingScheme = "EXTERNAL_MANAGED"
    forwardingRules     = [google_compute_global_forwarding_rule.lb.self_link]
    extensionChains = [
      for chain in local.chains : {
        name           = chain.name
        matchCondition = { celExpression = chain.match }
        extensions = [{
          name      = "${chain.name}-gateway"
          service   = google_compute_backend_service.backend[chain.backend].self_link
          authority = trimprefix(google_cloud_run_v2_service.gateway.uri, "https://")
          # Full duplex needs the trailers too.
          supportedEvents     = ["REQUEST_HEADERS", "REQUEST_BODY", "REQUEST_TRAILERS"]
          requestBodySendMode = chain.mode
          timeout             = format("%.3fs", var.timeout_ms / 1000)
          failOpen            = false
        }]
      }
    ]
  }
}

resource "terraform_data" "traffic_extension" {
  # Replaced on any change, so the import runs again.
  triggers_replace = local.extension
  input = {
    name   = local.extension_name
    source = jsonencode(local.extension)
  }

  # JSON is YAML, so gcloud reads it from stdin as is.
  provisioner "local-exec" {
    interpreter = ["bash", "-c"]
    command     = <<-EOT
      set -euo pipefail
      printf '%s' "$SOURCE" | gcloud service-extensions lb-traffic-extensions import "$NAME" --quiet
    EOT
    environment = {
      NAME   = self.input.name
      SOURCE = self.input.source
    }
  }

  provisioner "local-exec" {
    when        = destroy
    interpreter = ["bash", "-c"]
    # Only NOT_FOUND is gone: on any other failure, Terraform must keep it in state.
    command = <<-EOT
      out=$(gcloud service-extensions lb-traffic-extensions delete "$NAME" --quiet 2>&1) && exit 0
      printf '%s\n' "$out" >&2
      grep -q NOT_FOUND <<<"$out"
    EOT
    environment = {
      NAME = self.input.name
    }
  }
}
