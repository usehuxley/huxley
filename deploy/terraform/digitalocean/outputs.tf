output "dns" {
  description = "Point host and portal_host here: CNAMEs to App Platform's default domain (it then issues certificates), or A records to the Droplet."
  value = var.shape == "app_platform" ? {
    names  = [var.host, var.portal_host]
    target = digitalocean_app.main[0].default_ingress
    } : {
    names  = [var.host, var.portal_host]
    target = digitalocean_droplet.compose[0].ipv4_address
  }
}

output "bootstrap" {
  description = "Set the instance up once, after it's running: it prints the first user's password."
  value       = var.shape == "app_platform" ? "doctl apps console ${digitalocean_app.main[0].id} api, then: huxley admin bootstrap --org-name 'Your Company' --email you@example.com --name 'Your Name' --preset service-desk" : "ssh root@${digitalocean_droplet.compose[0].ipv4_address} 'cd /opt/huxley/deploy/compose && docker compose exec huxley huxley admin bootstrap --org-name \"Your Company\" --email you@example.com --name \"Your Name\" --preset service-desk'"
}

output "kek" {
  description = "The key-encryption key (App Platform). Back it up with the database: encrypted data can't be recovered without it."
  value       = random_bytes.kek.base64
  sensitive   = true
}
