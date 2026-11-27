terraform {
  required_version = ">= 1.6"
  required_providers {
    digitalocean = { source = "digitalocean/digitalocean", version = "~> 2.46" }
    random       = { source = "hashicorp/random", version = "~> 3.6" }
  }
}

# DIGITALOCEAN_TOKEN, plus SPACES_ACCESS_KEY_ID / SPACES_SECRET_ACCESS_KEY for
# creating the Spaces bucket.
provider "digitalocean" {}
