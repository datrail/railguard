# Inputs. Values are in terraform.tfvars.

variable "project_id" {
  description = "GCP project ID."
  type        = string
}

variable "region" {
  description = "Region for the Cloud Run services, their NEGs and the registry."
  type        = string
}

variable "prefix" {
  description = "Prefix for every resource name we choose."
  type        = string
}

variable "timeout_ms" {
  # Past the gateway's 5 s deadline, so its answer is the one sent.
  description = "The traffic extension's timeout for each message on the stream."
  type        = number
  default     = 6000
}
