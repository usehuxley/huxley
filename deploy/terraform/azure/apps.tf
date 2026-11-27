# Container Apps: a router (the only public ingress) in front of the API,
# web app and portal; worker pools by job class and the scheduler.

resource "azurerm_container_app_environment" "main" {
  name                       = "cae-${var.name}"
  resource_group_name        = azurerm_resource_group.main.name
  location                   = var.location
  log_analytics_workspace_id = azurerm_log_analytics_workspace.main.id
  infrastructure_subnet_id   = azurerm_subnet.apps.id
  zone_redundancy_enabled    = true
  workload_profile {
    name                  = "Consumption"
    workload_profile_type = "Consumption"
  }
  tags = local.tags
}

locals {
  pg_host = azurerm_postgresql_flexible_server.main.fqdn
  image   = { for app in ["huxley", "web", "portal"] : app => "${var.image_registry}/${app}:${var.image_tag}" }

  # The router stamps the client's address into this header (replacing
  # whatever a client sent); Huxley and the web apps read it.
  client_ip_header = "X-Huxley-Client-IP"

  huxley_env = merge({
    HUXLEY_DATABASE__URL                                = "postgres://huxley@${local.pg_host}:5432/huxley?sslmode=require"
    HUXLEY_MIGRATE_ON_START                             = "true"
    HUXLEY_CRYPTO__AZURE_KEY_VAULT__VAULT_URL           = azurerm_key_vault.main.vault_uri
    HUXLEY_CRYPTO__AZURE_KEY_VAULT__KEY                 = azurerm_key_vault_key.kek.name
    HUXLEY_CRYPTO__AZURE_KEY_VAULT__AUTH__CLIENT_ID     = azurerm_user_assigned_identity.apps.client_id
    AZURE_CLIENT_ID                                     = azurerm_user_assigned_identity.apps.client_id
    HUXLEY_STORAGE__URL                                 = "az://${azurerm_storage_container.objects.name}"
    HUXLEY_STORAGE__OPTIONS__AZURE_STORAGE_ACCOUNT_NAME = azurerm_storage_account.objects.name
    HUXLEY_HTTP__TRUST_FORWARDED_FOR                    = "true"
    HUXLEY_HTTP__CLIENT_IP_HEADER                       = local.client_ip_header
    HUXLEY_AUTH__PUBLIC_URL                             = "https://${var.host}"
    HUXLEY_HTTP__PORTAL_PUBLIC_URL                      = "https://${var.portal_host}"
    HUXLEY_EMAIL__FROM                                  = var.email_from
    HUXLEY_TELEMETRY__LOG_FORMAT                        = "json"
    HUXLEY_TELEMETRY__ENVIRONMENT                       = var.name
    }, var.otlp_endpoint == "" ? {} : {
    HUXLEY_TELEMETRY__OTLP_ENDPOINT = var.otlp_endpoint
  })

  secrets = merge(
    { pgpassword = random_password.postgres.result },
    var.smtp_url == "" ? {} : { smtp-url = var.smtp_url },
  )
  secret_env = merge(
    { PGPASSWORD = "pgpassword" },
    var.smtp_url == "" ? {} : { HUXLEY_EMAIL__SMTP_URL = "smtp-url" },
  )

  servers = merge(
    {
      api       = { roles = "api", min = var.api.min, max = var.api.max, cpu = var.api.cpu, memory = var.api.memory, env = {}, scale = null }
      scheduler = { roles = "scheduler", min = 1, max = 1, cpu = 0.25, memory = "0.5Gi", env = {}, scale = null }
    },
    { for name, pool in var.worker_pools : "worker-${name}" => {
      roles = "worker", min = pool.min, max = pool.max, cpu = pool.cpu, memory = pool.memory
      env = merge(
        { HUXLEY_ENGINE__CONCURRENCY = tostring(pool.concurrency) },
        length(pool.kinds) == 0 ? {} : { HUXLEY_ENGINE__KINDS = "[${join(",", pool.kinds)}]" },
      )
      # Due and running jobs of the pool's kinds (as the Helm chart's KEDA
      # trigger counts them); a replica runs `concurrency` at once.
      scale = {
        concurrency = pool.concurrency
        query = join(" ", compact([
          "SELECT count(*) FROM jobs WHERE NOT dead AND run_at <= now()",
          length(pool.kinds) == 0 ? "" : "AND kind IN (${join(", ", [for k in pool.kinds : "'${k}'"])})",
        ]))
      }
    } },
  )

  registry = var.acr_id == "" ? [] : [split("/", var.acr_id)[8]]
}

resource "azurerm_container_app" "server" {
  for_each                     = local.servers
  name                         = "${var.name}-${each.key}"
  resource_group_name          = azurerm_resource_group.main.name
  container_app_environment_id = azurerm_container_app_environment.main.id
  revision_mode                = "Single"
  workload_profile_name        = "Consumption"
  tags                         = local.tags

  identity {
    type         = "UserAssigned"
    identity_ids = [azurerm_user_assigned_identity.apps.id]
  }

  dynamic "registry" {
    for_each = local.registry
    content {
      server   = "${registry.value}.azurecr.io"
      identity = azurerm_user_assigned_identity.apps.id
    }
  }

  dynamic "secret" {
    for_each = local.secrets
    content {
      name  = secret.key
      value = secret.value
    }
  }

  # The API is reached inside the environment at http://<name>-api.
  dynamic "ingress" {
    for_each = each.key == "api" ? [1] : []
    content {
      external_enabled = false
      target_port      = 8080
      transport        = "http"
      traffic_weight {
        latest_revision = true
        percentage      = 100
      }
    }
  }

  template {
    min_replicas = each.value.min
    max_replicas = each.value.max
    container {
      name   = "huxley"
      image  = local.image.huxley
      cpu    = each.value.cpu
      memory = each.value.memory
      args   = ["serve", "--roles", each.value.roles]
      dynamic "env" {
        for_each = merge(local.huxley_env, each.value.env)
        content {
          name  = env.key
          value = env.value
        }
      }
      dynamic "env" {
        for_each = local.secret_env
        content {
          name        = env.key
          secret_name = env.value
        }
      }
      dynamic "readiness_probe" {
        for_each = each.key == "api" ? [1] : []
        content {
          transport = "HTTP"
          port      = 8080
          path      = "/readyz"
        }
      }
      dynamic "liveness_probe" {
        for_each = each.key == "api" ? [1] : []
        content {
          transport = "HTTP"
          port      = 8080
          path      = "/healthz"
        }
      }
    }
    dynamic "custom_scale_rule" {
      for_each = each.value.scale == null ? [] : [each.value.scale]
      content {
        name             = "job-backlog"
        custom_rule_type = "postgresql"
        metadata = {
          host             = local.pg_host
          port             = "5432"
          userName         = "huxley"
          dbName           = "huxley"
          sslmode          = "require"
          query            = custom_scale_rule.value.query
          targetQueryValue = tostring(custom_scale_rule.value.concurrency)
        }
        authentication {
          secret_name       = "pgpassword"
          trigger_parameter = "password"
        }
      }
    }
  }

  depends_on = [azurerm_role_assignment.kek, azurerm_role_assignment.blob, azurerm_postgresql_flexible_server_configuration.extensions]
}

resource "azurerm_container_app" "frontend" {
  for_each                     = { web = var.host, portal = var.portal_host }
  name                         = "${var.name}-${each.key}"
  resource_group_name          = azurerm_resource_group.main.name
  container_app_environment_id = azurerm_container_app_environment.main.id
  revision_mode                = "Single"
  workload_profile_name        = "Consumption"
  tags                         = local.tags

  identity {
    type         = "UserAssigned"
    identity_ids = [azurerm_user_assigned_identity.apps.id]
  }

  dynamic "registry" {
    for_each = local.registry
    content {
      server   = "${registry.value}.azurecr.io"
      identity = azurerm_user_assigned_identity.apps.id
    }
  }

  ingress {
    external_enabled = false
    target_port      = 3000
    transport        = "http"
    traffic_weight {
      latest_revision = true
      percentage      = 100
    }
  }

  template {
    min_replicas = var.frontend.min
    max_replicas = var.frontend.max
    container {
      name   = each.key
      image  = local.image[each.key]
      cpu    = var.frontend.cpu
      memory = var.frontend.memory
      env {
        name  = "HUXLEY_API_URL"
        value = "http://${var.name}-api"
      }
      env {
        name  = "ORIGIN"
        value = "https://${each.value}"
      }
      env {
        name  = "ADDRESS_HEADER"
        value = local.client_ip_header
      }
      env {
        name  = "BODY_SIZE_LIMIT"
        value = "30M"
      }
      liveness_probe {
        transport = "HTTP"
        port      = 3000
        path      = "/healthz"
      }
    }
  }
}

# The router: Caddy, routing as the Compose Caddyfile does, and setting the
# client-address header from the ingress's X-Forwarded-For.
locals {
  caddyfile = <<-CADDY
    {
    	admin off
    	auto_https off
    	servers {
    		trusted_proxies static private_ranges
    		client_ip_headers X-Forwarded-For
    	}
    }
    :8080 {
    	request_header -${local.client_ip_header}
    	@portal host ${var.portal_host}
    	handle @portal {
    		reverse_proxy http://${var.name}-portal {
    			header_up ${local.client_ip_header} {client_ip}
    		}
    	}
    	@api path /api/* /hooks/* /scim/* /callbacks/* /mcp /mcp/* /.well-known/oauth-* /oauth/register /oauth/token /oauth/revoke
    	handle @api {
    		reverse_proxy http://${var.name}-api {
    			header_up ${local.client_ip_header} {client_ip}
    		}
    	}
    	handle {
    		reverse_proxy http://${var.name}-web {
    			header_up ${local.client_ip_header} {client_ip}
    		}
    	}
    }
  CADDY
}

resource "azurerm_container_app" "router" {
  name                         = "${var.name}-router"
  resource_group_name          = azurerm_resource_group.main.name
  container_app_environment_id = azurerm_container_app_environment.main.id
  revision_mode                = "Single"
  workload_profile_name        = "Consumption"
  tags                         = local.tags

  ingress {
    external_enabled = true
    target_port      = 8080
    transport        = "http"
    traffic_weight {
      latest_revision = true
      percentage      = 100
    }
  }

  template {
    min_replicas = 2
    max_replicas = 10
    container {
      name    = "caddy"
      image   = "caddy:2.9"
      cpu     = 0.25
      memory  = "0.5Gi"
      command = ["/bin/sh", "-c", "printf '%s' \"$CADDYFILE\" > /tmp/Caddyfile && exec caddy run --config /tmp/Caddyfile --adapter caddyfile"]
      env {
        name  = "CADDYFILE"
        value = local.caddyfile
      }
      liveness_probe {
        transport = "TCP"
        port      = 8080
      }
    }
  }
}
