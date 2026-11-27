terraform {
  required_version = ">= 1.6"
  required_providers {
    azurerm = { source = "hashicorp/azurerm", version = "~> 4.14" }
    random  = { source = "hashicorp/random", version = "~> 3.6" }
  }
}

provider "azurerm" {
  features {
    key_vault {
      # The key-encryption key: never purge it with the deployment.
      purge_soft_delete_on_destroy = false
    }
  }
}
