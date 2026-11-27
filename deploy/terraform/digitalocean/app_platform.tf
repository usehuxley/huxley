# Huxley on App Platform (ADR-092): Managed PostgreSQL, Spaces for objects,
# and the key-encryption key as an encrypted app secret (DigitalOcean has no
# key management service; point crypto.vault at a Vault server for one).

locals {
  app = var.shape == "app_platform"
}

resource "digitalocean_project" "main" {
  name        = var.name
  description = "Huxley"
  purpose     = "Web Application"
  environment = "Production"
}

resource "digitalocean_database_cluster" "main" {
  count      = local.app ? 1 : 0
  name       = "${var.name}-db"
  engine     = "pg"
  version    = var.database.version
  size       = var.database.size
  region     = var.region
  node_count = var.database.node_count
}

resource "digitalocean_database_db" "huxley" {
  count      = local.app ? 1 : 0
  cluster_id = digitalocean_database_cluster.main[0].id
  name       = "huxley"
}

resource "digitalocean_database_user" "huxley" {
  count      = local.app ? 1 : 0
  cluster_id = digitalocean_database_cluster.main[0].id
  name       = "huxley"
}

# Only the app may connect.
resource "digitalocean_database_firewall" "main" {
  count      = local.app ? 1 : 0
  cluster_id = digitalocean_database_cluster.main[0].id
  rule {
    type  = "app"
    value = digitalocean_app.main[0].id
  }
}

resource "digitalocean_spaces_bucket" "objects" {
  count  = local.app ? 1 : 0
  name   = "${var.name}-objects-${random_id.suffix.hex}"
  region = var.region
  acl    = "private"
  versioning {
    enabled = true
  }
}

resource "random_id" "suffix" {
  byte_length = 3
}

resource "digitalocean_spaces_key" "objects" {
  count = local.app ? 1 : 0
  name  = "${var.name}-objects"
  grant {
    bucket     = digitalocean_spaces_bucket.objects[0].name
    permission = "readwrite"
  }
}

resource "random_bytes" "kek" {
  length = 32
}

locals {
  image_block = { registry_type = var.image.registry_type, registry = var.image.registry, tag = var.image.tag }

  plain_env = {
    HUXLEY_MIGRATE_ON_START               = "true"
    HUXLEY_STORAGE__URL                   = local.app ? "s3://${digitalocean_spaces_bucket.objects[0].name}" : ""
    HUXLEY_STORAGE__OPTIONS__AWS_ENDPOINT = "https://${var.region}.digitaloceanspaces.com"
    HUXLEY_STORAGE__OPTIONS__AWS_REGION   = "us-east-1"
    HUXLEY_HTTP__TRUST_FORWARDED_FOR      = "true"
    # App Platform puts the client's address in this header.
    HUXLEY_HTTP__CLIENT_IP_HEADER  = "DO-Connecting-IP"
    HUXLEY_AUTH__PUBLIC_URL        = "https://${var.host}"
    HUXLEY_HTTP__PORTAL_PUBLIC_URL = "https://${var.portal_host}"
    HUXLEY_EMAIL__FROM             = var.email_from
    HUXLEY_TELEMETRY__LOG_FORMAT   = "json"
  }
  secret_env = merge(
    {
      HUXLEY_CRYPTO__KEK                             = random_bytes.kek.base64
      HUXLEY_STORAGE__OPTIONS__AWS_ACCESS_KEY_ID     = local.app ? digitalocean_spaces_key.objects[0].access_key : ""
      HUXLEY_STORAGE__OPTIONS__AWS_SECRET_ACCESS_KEY = local.app ? digitalocean_spaces_key.objects[0].secret_key : ""
    },
    var.smtp_url == "" ? {} : { HUXLEY_EMAIL__SMTP_URL = var.smtp_url },
  )

  # The API, webhooks, SCIM, MCP and its OAuth endpoints go to the API (as
  # the Compose Caddyfile routes them); the rest of the staff host to the
  # web app; the portal host to the portal.
  api_prefixes = [
    "/api", "/hooks", "/scim", "/callbacks", "/mcp",
    "/.well-known/oauth-protected-resource", "/.well-known/oauth-authorization-server",
    "/oauth/register", "/oauth/token", "/oauth/revoke",
  ]
}

resource "digitalocean_app" "main" {
  count      = local.app ? 1 : 0
  project_id = digitalocean_project.main.id

  spec {
    name   = var.name
    region = var.app_region

    domain {
      name = var.host
      type = "PRIMARY"
    }
    domain {
      name = var.portal_host
      type = "ALIAS"
    }

    database {
      name         = "db"
      engine       = "PG"
      production   = true
      cluster_name = digitalocean_database_cluster.main[0].name
      db_name      = digitalocean_database_db.huxley[0].name
      db_user      = digitalocean_database_user.huxley[0].name
    }

    ingress {
      rule {
        component {
          name = "portal"
        }
        match {
          authority {
            exact = var.portal_host
          }
          path {
            prefix = "/"
          }
        }
      }
      dynamic "rule" {
        for_each = local.api_prefixes
        content {
          component {
            name                 = "api"
            preserve_path_prefix = true
          }
          match {
            path {
              prefix = rule.value
            }
          }
        }
      }
      rule {
        component {
          name = "web"
        }
        match {
          path {
            prefix = "/"
          }
        }
      }
    }

    service {
      name               = "api"
      instance_count     = var.api_count
      instance_size_slug = var.instance_size
      http_port          = 8080
      image {
        registry_type = local.image_block.registry_type
        registry      = local.image_block.registry
        repository    = "huxley"
        tag           = local.image_block.tag
      }
      health_check {
        http_path             = "/readyz"
        initial_delay_seconds = 10
      }
      # The image is distroless (no shell for a run command): roles by
      # variable.
      env {
        key   = "HUXLEY_ROLES"
        value = "api"
      }
      env {
        key   = "HUXLEY_DATABASE__URL"
        value = "$${db.DATABASE_URL}"
      }
      dynamic "env" {
        for_each = local.plain_env
        content {
          key   = env.key
          value = env.value
        }
      }
      dynamic "env" {
        for_each = local.secret_env
        content {
          key   = env.key
          value = env.value
          type  = "SECRET"
        }
      }
    }

    dynamic "worker" {
      for_each = merge(
        { scheduler = { roles = "scheduler", count = 1, env = {} } },
        { for name, pool in var.worker_pools : "worker-${name}" => {
          roles = "worker", count = pool.count
          env = merge(
            { HUXLEY_ENGINE__CONCURRENCY = tostring(pool.concurrency) },
            length(pool.kinds) == 0 ? {} : { HUXLEY_ENGINE__KINDS = "[${join(",", pool.kinds)}]" },
          )
        } },
      )
      content {
        name               = worker.key
        instance_count     = worker.value.count
        instance_size_slug = var.instance_size
        image {
          registry_type = local.image_block.registry_type
          registry      = local.image_block.registry
          repository    = "huxley"
          tag           = local.image_block.tag
        }
        env {
          key   = "HUXLEY_ROLES"
          value = worker.value.roles
        }
        env {
          key   = "HUXLEY_DATABASE__URL"
          value = "$${db.DATABASE_URL}"
        }
        dynamic "env" {
          for_each = merge(local.plain_env, worker.value.env)
          content {
            key   = env.key
            value = env.value
          }
        }
        dynamic "env" {
          for_each = local.secret_env
          content {
            key   = env.key
            value = env.value
            type  = "SECRET"
          }
        }
      }
    }

    dynamic "service" {
      for_each = { web = var.host, portal = var.portal_host }
      content {
        name               = service.key
        instance_count     = 2
        instance_size_slug = var.instance_size
        http_port          = 3000
        image {
          registry_type = local.image_block.registry_type
          registry      = local.image_block.registry
          repository    = service.key
          tag           = local.image_block.tag
        }
        health_check {
          http_path = "/healthz"
        }
        env {
          key   = "HUXLEY_API_URL"
          value = "$${api.PRIVATE_URL}"
        }
        env {
          key   = "ORIGIN"
          value = "https://${service.value}"
        }
        env {
          key   = "ADDRESS_HEADER"
          value = "DO-Connecting-IP"
        }
        env {
          key   = "BODY_SIZE_LIMIT"
          value = "30M"
        }
      }
    }
  }
}
