terraform {
  required_version = ">= 1.6"
  required_providers {
    aws    = { source = "hashicorp/aws", version = "~> 5.80" }
    random = { source = "hashicorp/random", version = "~> 3.6" }
  }
}

provider "aws" {
  region = var.region
  default_tags {
    tags = merge({ "app" = "huxley", "huxley:deployment" = var.name }, var.tags)
  }
}

# CloudFront certificates live in us-east-1.
provider "aws" {
  alias  = "us_east_1"
  region = "us-east-1"
}
