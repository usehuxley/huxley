variable "name" {
  description = "Name for this deployment; prefixes every resource."
  type        = string
  default     = "huxley"
  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{1,24}$", var.name))
    error_message = "Lowercase letters, digits and hyphens, up to 25 characters."
  }
}

variable "shape" {
  description = "app_platform (managed: App Platform, Managed PostgreSQL, Spaces) or droplet (one server running the Compose deployment: small installs)."
  type        = string
  default     = "app_platform"
  validation {
    condition     = contains(["app_platform", "droplet"], var.shape)
    error_message = "app_platform or droplet."
  }
}

variable "region" {
  description = "Datacenter for the database, Spaces and Droplet, e.g. nyc3, ams3, sgp1."
  type        = string
  default     = "nyc3"
}

variable "app_region" {
  description = "App Platform region, e.g. nyc, ams, sgp (match region)."
  type        = string
  default     = "nyc"
}

variable "host" {
  description = "The staff app's host name, e.g. huxley.example.com."
  type        = string
}

variable "portal_host" {
  description = "The client portal's host name, e.g. portal.example.com."
  type        = string
}

variable "image" {
  description = "Where App Platform pulls images from (no images are published yet: build and push them, e.g. to DigitalOcean Container Registry)."
  type = object({
    registry_type = optional(string, "DOCR") # DOCR, GHCR or DOCKER_HUB
    registry      = optional(string, "")     # GHCR/Docker Hub: the owner, e.g. usehuxley
    tag           = optional(string, "latest")
  })
  default = {}
}

variable "database" {
  description = "Managed PostgreSQL size and nodes (2+ for a standby)."
  type = object({
    size       = optional(string, "db-s-2vcpu-4gb")
    node_count = optional(number, 2)
    version    = optional(string, "16")
  })
  default = {}
}

variable "instance_size" {
  description = "App Platform size for the API, workers and web apps."
  type        = string
  default     = "apps-s-1vcpu-1gb"
}

variable "api_count" {
  type    = number
  default = 2
}

variable "worker_pools" {
  description = "Worker pools by job class (ADR-089): job kinds, jobs at once per instance, instance count."
  type = map(object({
    kinds       = list(string)
    concurrency = number
    count       = number
  }))
  default = {
    steps = { kinds = ["step"], concurrency = 16, count = 2 }
    runs  = { kinds = ["advance", "deadline"], concurrency = 16, count = 1 }
  }
}

variable "smtp_url" {
  type      = string
  default   = ""
  sensitive = true
}

variable "email_from" {
  type    = string
  default = "Huxley <huxley@example.com>"
}

variable "droplet" {
  description = "For shape = droplet: size, the repository and ref to build from, SSH keys, and who may SSH in."
  type = object({
    size        = optional(string, "s-4vcpu-8gb") # Building from source needs the memory.
    repo        = optional(string, "https://github.com/usehuxley/huxley.git")
    ref         = optional(string, "main")
    ssh_keys    = optional(list(string), [])
    ssh_sources = optional(list(string), [])
  })
  default = {}
}
