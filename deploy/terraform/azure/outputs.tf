output "router_fqdn" {
  description = "Point host and portal_host here (CNAME), then add them as custom domains on the router app (see the README)."
  value       = azurerm_container_app.router.ingress[0].fqdn
}

output "domain_verification_id" {
  description = "For the asuid.<host> TXT records Container Apps asks for when adding custom domains."
  value       = azurerm_container_app_environment.main.custom_domain_verification_id
}

output "bootstrap" {
  description = "Set the instance up once, after the API is healthy: it prints the first user's password."
  value       = "az containerapp exec --resource-group ${azurerm_resource_group.main.name} --name ${azurerm_container_app.server["api"].name} --command \"huxley admin bootstrap --org-name 'Your Company' --email you@example.com --name 'Your Name' --preset service-desk\""
}

output "key_vault_key_id" {
  description = "The key-encryption key. Huxley's data keys are useless without it: don't delete it."
  value       = azurerm_key_vault_key.kek.versionless_id
}

output "resource_group" {
  value = azurerm_resource_group.main.name
}
