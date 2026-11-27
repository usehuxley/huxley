# Huxley on Azure Container Apps (ADR-092): PostgreSQL Flexible Server on a
# private network, Blob Storage, and the key-encryption key in Key Vault,
# reached with a managed identity.

data "azurerm_client_config" "current" {}

locals {
  tags = merge({ app = "huxley", "huxley-deployment" = var.name }, var.tags)
}

resource "azurerm_resource_group" "main" {
  name     = "rg-${var.name}"
  location = var.location
  tags     = local.tags
}

resource "azurerm_log_analytics_workspace" "main" {
  name                = "log-${var.name}"
  resource_group_name = azurerm_resource_group.main.name
  location            = var.location
  sku                 = "PerGB2018"
  retention_in_days   = 30
  tags                = local.tags
}

# --- Network ---------------------------------------------------------------

resource "azurerm_virtual_network" "main" {
  name                = "vnet-${var.name}"
  resource_group_name = azurerm_resource_group.main.name
  location            = var.location
  address_space       = [var.vnet_cidr]
  tags                = local.tags
}

resource "azurerm_subnet" "apps" {
  name                 = "apps"
  resource_group_name  = azurerm_resource_group.main.name
  virtual_network_name = azurerm_virtual_network.main.name
  address_prefixes     = [cidrsubnet(var.vnet_cidr, 7, 0)]
  delegation {
    name = "container-apps"
    service_delegation {
      name    = "Microsoft.App/environments"
      actions = ["Microsoft.Network/virtualNetworks/subnets/join/action"]
    }
  }
}

resource "azurerm_subnet" "db" {
  name                 = "db"
  resource_group_name  = azurerm_resource_group.main.name
  virtual_network_name = azurerm_virtual_network.main.name
  address_prefixes     = [cidrsubnet(var.vnet_cidr, 8, 10)]
  service_endpoints    = ["Microsoft.Storage"]
  delegation {
    name = "postgres"
    service_delegation {
      name    = "Microsoft.DBforPostgreSQL/flexibleServers"
      actions = ["Microsoft.Network/virtualNetworks/subnets/join/action"]
    }
  }
}

resource "azurerm_private_dns_zone" "postgres" {
  name                = "${var.name}.private.postgres.database.azure.com"
  resource_group_name = azurerm_resource_group.main.name
  tags                = local.tags
}

resource "azurerm_private_dns_zone_virtual_network_link" "postgres" {
  name                  = "postgres"
  resource_group_name   = azurerm_resource_group.main.name
  private_dns_zone_name = azurerm_private_dns_zone.postgres.name
  virtual_network_id    = azurerm_virtual_network.main.id
}

# --- PostgreSQL --------------------------------------------------------------

resource "random_password" "postgres" {
  length  = 40
  special = false
}

resource "azurerm_postgresql_flexible_server" "main" {
  name                          = "psql-${var.name}"
  resource_group_name           = azurerm_resource_group.main.name
  location                      = var.location
  version                       = var.postgres.version
  sku_name                      = var.postgres.sku
  storage_mb                    = var.postgres.storage_mb
  auto_grow_enabled             = true
  backup_retention_days         = 14
  delegated_subnet_id           = azurerm_subnet.db.id
  private_dns_zone_id           = azurerm_private_dns_zone.postgres.id
  public_network_access_enabled = false
  administrator_login           = "huxley"
  administrator_password        = random_password.postgres.result
  zone                          = "1"
  dynamic "high_availability" {
    for_each = var.postgres.ha ? [1] : []
    content {
      mode                      = "ZoneRedundant"
      standby_availability_zone = "2"
    }
  }
  tags       = local.tags
  depends_on = [azurerm_private_dns_zone_virtual_network_link.postgres]
}

# The extensions Huxley's migrations create.
resource "azurerm_postgresql_flexible_server_configuration" "extensions" {
  name      = "azure.extensions"
  server_id = azurerm_postgresql_flexible_server.main.id
  value     = "LTREE,PGCRYPTO,PG_TRGM,VECTOR"
}

resource "azurerm_postgresql_flexible_server_database" "huxley" {
  name      = "huxley"
  server_id = azurerm_postgresql_flexible_server.main.id
  charset   = "UTF8"
  collation = "en_US.utf8"
}

# --- Identity, storage, key ------------------------------------------------

resource "azurerm_user_assigned_identity" "apps" {
  name                = "id-${var.name}"
  resource_group_name = azurerm_resource_group.main.name
  location            = var.location
  tags                = local.tags
}

resource "azurerm_storage_account" "objects" {
  name                            = "st${var.name}${substr(md5(azurerm_resource_group.main.id), 0, 6)}"
  resource_group_name             = azurerm_resource_group.main.name
  location                        = var.location
  account_tier                    = "Standard"
  account_replication_type        = "ZRS"
  min_tls_version                 = "TLS1_2"
  allow_nested_items_to_be_public = false
  shared_access_key_enabled       = false # The apps use their managed identity.
  default_to_oauth_authentication = true
  blob_properties {
    versioning_enabled = true
    delete_retention_policy {
      days = 14
    }
  }
  tags = local.tags
}

resource "azurerm_storage_container" "objects" {
  name               = "objects"
  storage_account_id = azurerm_storage_account.objects.id
}

resource "azurerm_role_assignment" "blob" {
  scope                = azurerm_storage_account.objects.id
  role_definition_name = "Storage Blob Data Contributor"
  principal_id         = azurerm_user_assigned_identity.apps.principal_id
}

resource "azurerm_key_vault" "main" {
  name                       = "kv-${var.name}-${substr(md5(azurerm_resource_group.main.id), 0, 6)}"
  resource_group_name        = azurerm_resource_group.main.name
  location                   = var.location
  tenant_id                  = data.azurerm_client_config.current.tenant_id
  sku_name                   = "premium"
  rbac_authorization_enabled = true
  purge_protection_enabled   = true
  soft_delete_retention_days = 90
  tags                       = local.tags
}

# Whoever runs Terraform creates the key...
resource "azurerm_role_assignment" "deployer_keys" {
  scope                = azurerm_key_vault.main.id
  role_definition_name = "Key Vault Crypto Officer"
  principal_id         = data.azurerm_client_config.current.object_id
}

# ...and Huxley may only wrap and unwrap with it (ADR-018).
resource "azurerm_key_vault_key" "kek" {
  name         = "huxley-kek"
  key_vault_id = azurerm_key_vault.main.id
  key_type     = "RSA-HSM"
  key_size     = 3072
  key_opts     = ["wrapKey", "unwrapKey"]
  depends_on   = [azurerm_role_assignment.deployer_keys]
}

resource "azurerm_role_assignment" "kek" {
  scope                = azurerm_key_vault_key.kek.resource_versionless_id
  role_definition_name = "Key Vault Crypto User"
  principal_id         = azurerm_user_assigned_identity.apps.principal_id
}

resource "azurerm_role_assignment" "acr" {
  count                = var.acr_id == "" ? 0 : 1
  scope                = var.acr_id
  role_definition_name = "AcrPull"
  principal_id         = azurerm_user_assigned_identity.apps.principal_id
}
