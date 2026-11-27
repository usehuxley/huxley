# One Droplet running the Compose deployment (deploy/compose): PostgreSQL,
# Huxley, the web apps and Caddy (automatic TLS) on one server. For small
# installs; it builds from source until images are published.

resource "digitalocean_droplet" "compose" {
  count      = var.shape == "droplet" ? 1 : 0
  name       = var.name
  image      = "ubuntu-24-04-x64"
  region     = var.region
  size       = var.droplet.size
  ssh_keys   = var.droplet.ssh_keys
  backups    = true
  monitoring = true
  user_data = templatefile("${path.module}/cloud-init.yaml.tftpl", {
    repo        = var.droplet.repo
    ref         = var.droplet.ref
    host        = var.host
    portal_host = var.portal_host
    email_from  = var.email_from
    smtp_url    = var.smtp_url
  })
}

resource "digitalocean_project_resources" "compose" {
  count     = var.shape == "droplet" ? 1 : 0
  project   = digitalocean_project.main.id
  resources = [digitalocean_droplet.compose[0].urn]
}

resource "digitalocean_firewall" "compose" {
  count       = var.shape == "droplet" ? 1 : 0
  name        = var.name
  droplet_ids = [digitalocean_droplet.compose[0].id]
  dynamic "inbound_rule" {
    for_each = length(var.droplet.ssh_sources) == 0 ? [] : [1]
    content {
      protocol         = "tcp"
      port_range       = "22"
      source_addresses = var.droplet.ssh_sources
    }
  }
  inbound_rule {
    protocol         = "tcp"
    port_range       = "80"
    source_addresses = ["0.0.0.0/0", "::/0"]
  }
  inbound_rule {
    protocol         = "tcp"
    port_range       = "443"
    source_addresses = ["0.0.0.0/0", "::/0"]
  }
  inbound_rule {
    protocol         = "udp"
    port_range       = "443"
    source_addresses = ["0.0.0.0/0", "::/0"]
  }
  outbound_rule {
    protocol              = "tcp"
    port_range            = "1-65535"
    destination_addresses = ["0.0.0.0/0", "::/0"]
  }
  outbound_rule {
    protocol              = "udp"
    port_range            = "1-65535"
    destination_addresses = ["0.0.0.0/0", "::/0"]
  }
}
