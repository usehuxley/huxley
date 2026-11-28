# Marketplace images for Huxley (deploy/marketplace/README.md): Ubuntu 24.04 with
# Docker and the Compose deployment (deploy/compose), its images already
# pulled. No secrets are baked in: each server makes its own on first
# boot, and `huxley-setup` (offered on first login) asks for the domain
# and the first user, then starts Huxley.
#
#   packer init deploy/marketplace
#   packer build -only='digitalocean.huxley' -var huxley_version=1.0.0 deploy/marketplace
#   packer build -only='azure-arm.huxley'    -var huxley_version=1.0.0 deploy/marketplace
#   packer build -only='amazon-ebs.huxley'   -var huxley_version=1.0.0 deploy/marketplace
#
# Building needs published images (ghcr.io/usehuxley/*:<huxley_version>).
# `-var prepull=false` leaves them out (setup pulls them), for trying the
# recipe before a release.

packer {
  required_plugins {
    digitalocean = {
      source  = "github.com/digitalocean/digitalocean"
      version = "= 1.4.1"
    }
    azure = {
      source  = "github.com/hashicorp/azure"
      version = "= 2.3.3"
    }
    amazon = {
      source  = "github.com/hashicorp/amazon"
      version = "= 1.3.9"
    }
  }
}

variable "huxley_version" {
  type        = string
  description = "The release whose images go in the image (a tag of ghcr.io/usehuxley/*)."
}

variable "prepull" {
  type        = bool
  default     = true
  description = "Pull Huxley's container images into the image, so a server starts without downloading them."
}

# DigitalOcean (Marketplace: a snapshot shared with the Marketplace team).
variable "do_token" {
  type      = string
  default   = env("DIGITALOCEAN_TOKEN")
  sensitive = true
}
variable "do_region" {
  type    = string
  default = "nyc3"
}

# Azure (Marketplace: a managed image or Compute Gallery version, published
# as a VM offer from Partner Center).
variable "azure_subscription_id" {
  type    = string
  default = env("ARM_SUBSCRIPTION_ID")
}
variable "azure_resource_group" {
  type    = string
  default = "huxley-images"
}
variable "azure_location" {
  type    = string
  default = "eastus"
}

# AWS (Marketplace: an AMI shared with the AWS Marketplace account).
variable "aws_region" {
  type    = string
  default = "us-east-1"
}

locals {
  name = "huxley-${replace(var.huxley_version, ".", "-")}-${formatdate("YYYYMMDDhhmm", timestamp())}"
}

source "digitalocean" "huxley" {
  api_token     = var.do_token
  image         = "ubuntu-24-04-x64"
  region        = var.do_region
  size          = "s-2vcpu-4gb"
  ssh_username  = "root"
  snapshot_name = local.name
}

source "azure-arm" "huxley" {
  use_azure_cli_auth                = true
  subscription_id                   = var.azure_subscription_id
  os_type                           = "Linux"
  image_publisher                   = "Canonical"
  image_offer                       = "ubuntu-24_04-lts"
  image_sku                         = "server"
  location                          = var.azure_location
  vm_size                           = "Standard_B2s"
  managed_image_name                = local.name
  managed_image_resource_group_name = var.azure_resource_group
}

source "amazon-ebs" "huxley" {
  region        = var.aws_region
  instance_type = "t3.medium"
  ssh_username  = "ubuntu"
  ami_name      = local.name
  source_ami_filter {
    filters = {
      name                = "ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-amd64-server-*"
      virtualization-type = "hvm"
    }
    owners      = ["099720109477"] # Canonical
    most_recent = true
  }
}

build {
  sources = [
    "source.digitalocean.huxley",
    "source.azure-arm.huxley",
    "source.amazon-ebs.huxley",
  ]

  # The Compose deployment, as in the repository at this version.
  provisioner "shell" {
    inline = ["mkdir -p /tmp/huxley/deploy"]
  }
  provisioner "file" {
    sources     = ["${path.root}/../compose", "${path.root}/../observability"]
    destination = "/tmp/huxley/deploy/"
  }
  provisioner "file" {
    source      = "${path.root}/files/"
    destination = "/tmp/huxley/"
  }

  provisioner "shell" {
    execute_command = "chmod +x {{ .Path }}; sudo env {{ .Vars }} {{ .Path }}"
    environment_vars = [
      "HUXLEY_VERSION=${var.huxley_version}",
      "PREPULL=${var.prepull}",
    ]
    scripts = [
      "${path.root}/scripts/install.sh",
      "${path.root}/scripts/cleanup.sh",
    ]
  }

  # Azure images are generalized last, as Azure requires.
  provisioner "shell" {
    only            = ["azure-arm.huxley"]
    execute_command = "chmod +x {{ .Path }}; sudo {{ .Path }}"
    inline          = ["/usr/sbin/waagent -force -deprovision+user && export HISTSIZE=0 && sync"]
  }
}
