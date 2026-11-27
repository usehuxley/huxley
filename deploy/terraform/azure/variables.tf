variable "name" {
  description = "Name for this deployment; prefixes every resource."
  type        = string
  default     = "huxley"
  validation {
    condition     = can(regex("^[a-z][a-z0-9]{1,15}$", var.name))
    error_message = "Lowercase letters and digits, up to 16 characters (storage account and Key Vault names are strict)."
  }
}

variable "location" {
  description = "Azure region, e.g. westeurope."
  type        = string
}

variable "host" {
  description = "The staff app's host name, e.g. huxley.example.com."
  type        = string
}

variable "portal_host" {
  description = "The client portal's host name, e.g. portal.example.com."
  type        = string
}

variable "image_registry" {
  description = "Where the huxley, web and portal images are (no images are published yet: build and push them, e.g. to Azure Container Registry)."
  type        = string
  default     = "ghcr.io/usehuxley"
}

variable "image_tag" {
  type    = string
  default = "latest"
}

variable "acr_id" {
  description = "Optional: an Azure Container Registry's resource id. The apps' identity gets AcrPull on it and pulls with it."
  type        = string
  default     = ""
}

variable "vnet_cidr" {
  type    = string
  default = "10.41.0.0/16"
}

variable "postgres" {
  description = "PostgreSQL Flexible Server: SKU, storage (MB), and zone-redundant high availability."
  type = object({
    sku        = optional(string, "GP_Standard_D2ds_v5")
    storage_mb = optional(number, 65536)
    version    = optional(string, "16")
    ha         = optional(bool, true)
  })
  default = {}
}

variable "api" {
  type    = object({ min = number, max = number, cpu = number, memory = string })
  default = { min = 2, max = 10, cpu = 0.5, memory = "1Gi" }
}

variable "worker_pools" {
  description = "Worker pools by job class (ADR-089): job kinds, jobs at once per replica, replica range, size. Each scales on its backlog (KEDA)."
  type = map(object({
    kinds       = list(string)
    concurrency = number
    min         = number
    max         = number
    cpu         = number
    memory      = string
  }))
  default = {
    steps = { kinds = ["step"], concurrency = 16, min = 1, max = 20, cpu = 0.5, memory = "1Gi" }
    runs  = { kinds = ["advance", "deadline"], concurrency = 16, min = 1, max = 6, cpu = 0.25, memory = "0.5Gi" }
  }
}

variable "frontend" {
  type    = object({ min = number, max = number, cpu = number, memory = string })
  default = { min = 1, max = 6, cpu = 0.25, memory = "0.5Gi" }
}

variable "smtp_url" {
  description = "Optional: smtp://user:pass@host:587 for Huxley's email."
  type        = string
  default     = ""
  sensitive   = true
}

variable "email_from" {
  type    = string
  default = "Huxley <huxley@example.com>"
}

variable "otlp_endpoint" {
  description = "Optional: an OpenTelemetry Collector's OTLP gRPC endpoint."
  type        = string
  default     = ""
}

variable "tags" {
  type    = map(string)
  default = {}
}
