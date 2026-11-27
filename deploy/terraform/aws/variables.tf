variable "name" {
  description = "Name for this deployment; prefixes every resource."
  type        = string
  default     = "huxley"
  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{1,20}$", var.name))
    error_message = "Lowercase letters, digits and hyphens, up to 21 characters."
  }
}

variable "region" {
  description = "AWS region, e.g. us-east-1."
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

variable "certificate_arn" {
  description = "An ACM certificate in this region covering host and portal_host, for the load balancer."
  type        = string
}

variable "image_registry" {
  description = "Where the huxley, web and portal images are (no images are published yet: build and push them, e.g. to ECR)."
  type        = string
  default     = "ghcr.io/usehuxley"
}

variable "image_tag" {
  description = "The image tag to run."
  type        = string
  default     = "latest"
}

variable "vpc_cidr" {
  description = "The VPC's address range."
  type        = string
  default     = "10.40.0.0/16"
}

variable "db_instance_class" {
  description = "RDS instance class."
  type        = string
  default     = "db.t4g.medium"
}

variable "db_allocated_storage" {
  description = "RDS storage in GB (grows automatically up to 5x)."
  type        = number
  default     = 50
}

variable "db_multi_az" {
  description = "A standby in a second availability zone (recommended in production)."
  type        = bool
  default     = true
}

variable "db_engine_version" {
  description = "PostgreSQL major version on RDS."
  type        = string
  default     = "16"
}

variable "api" {
  description = "API tasks: count, CPU units and memory (MiB)."
  type        = object({ count = number, cpu = number, memory = number })
  default     = { count = 2, cpu = 512, memory = 1024 }
}

variable "worker_pools" {
  description = "Worker pools by job class (ADR-089): which job kinds each takes, jobs at once per task, task count and size."
  type = map(object({
    kinds       = list(string)
    concurrency = number
    count       = number
    cpu         = number
    memory      = number
  }))
  default = {
    steps = { kinds = ["step"], concurrency = 16, count = 2, cpu = 512, memory = 1024 }
    runs  = { kinds = ["advance", "deadline"], concurrency = 16, count = 1, cpu = 256, memory = 512 }
  }
}

variable "scheduler" {
  description = "Scheduler task size. One task (two are safe, for availability)."
  type        = object({ count = number, cpu = number, memory = number })
  default     = { count = 1, cpu = 256, memory = 512 }
}

variable "frontend" {
  description = "Web app and portal tasks: count, CPU units and memory (MiB) each."
  type        = object({ count = number, cpu = number, memory = number })
  default     = { count = 2, cpu = 256, memory = 512 }
}

variable "smtp_url_secret_arn" {
  description = "Optional: a Secrets Manager secret holding the SMTP URL (smtp://user:pass@host:587)."
  type        = string
  default     = ""
}

variable "email_from" {
  description = "The From address on Huxley's email."
  type        = string
  default     = "Huxley <huxley@example.com>"
}

variable "cloudfront" {
  description = "Put CloudFront in front of the load balancer. The load balancer then only accepts requests carrying CloudFront's secret header (origin lock)."
  type = object({
    enabled         = bool
    certificate_arn = optional(string, "") # In us-east-1, covering host and portal_host.
    price_class     = optional(string, "PriceClass_100")
  })
  default = { enabled = false }
}

variable "otlp_endpoint" {
  description = "Optional: an OpenTelemetry Collector's OTLP gRPC endpoint for traces, metrics and logs."
  type        = string
  default     = ""
}

variable "log_retention_days" {
  description = "CloudWatch log retention."
  type        = number
  default     = 30
}

variable "deletion_protection" {
  description = "Protect the database and load balancer from deletion. Turn off only to tear a test deployment down."
  type        = bool
  default     = true
}

variable "tags" {
  description = "Extra tags on every resource."
  type        = map(string)
  default     = {}
}
