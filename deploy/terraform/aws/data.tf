# PostgreSQL, object storage, and the key-encryption key.

data "aws_caller_identity" "current" {}
data "aws_partition" "current" {}

resource "aws_db_subnet_group" "main" {
  name       = var.name
  subnet_ids = aws_subnet.private[*].id
}

resource "aws_db_parameter_group" "main" {
  name   = "${var.name}-pg${var.db_engine_version}"
  family = "postgres${var.db_engine_version}"
  parameter {
    name  = "rds.force_ssl"
    value = "1"
  }
}

resource "aws_db_instance" "main" {
  identifier                   = var.name
  engine                       = "postgres"
  engine_version               = var.db_engine_version
  instance_class               = var.db_instance_class
  allocated_storage            = var.db_allocated_storage
  max_allocated_storage        = var.db_allocated_storage * 5
  storage_type                 = "gp3"
  storage_encrypted            = true
  db_name                      = "huxley"
  username                     = "huxley"
  manage_master_user_password  = true # Kept in Secrets Manager, rotated by RDS.
  db_subnet_group_name         = aws_db_subnet_group.main.name
  parameter_group_name         = aws_db_parameter_group.main.name
  vpc_security_group_ids       = [aws_security_group.db.id]
  multi_az                     = var.db_multi_az
  backup_retention_period      = 14
  deletion_protection          = var.deletion_protection
  skip_final_snapshot          = !var.deletion_protection
  final_snapshot_identifier    = var.deletion_protection ? "${var.name}-final" : null
  copy_tags_to_snapshot        = true
  auto_minor_version_upgrade   = true
  performance_insights_enabled = true
}

# Attachments and spilled workflow payloads.
resource "aws_s3_bucket" "objects" {
  bucket_prefix = "${var.name}-objects-"
  force_destroy = !var.deletion_protection
}

resource "aws_s3_bucket_public_access_block" "objects" {
  bucket                  = aws_s3_bucket.objects.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_ownership_controls" "objects" {
  bucket = aws_s3_bucket.objects.id
  rule {
    object_ownership = "BucketOwnerEnforced"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "objects" {
  bucket = aws_s3_bucket.objects.id
  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_versioning" "objects" {
  bucket = aws_s3_bucket.objects.id
  versioning_configuration {
    status = "Enabled"
  }
}

# The key-encryption key (ADR-092): Huxley asks KMS to wrap and unwrap its
# data keys; the key never leaves KMS. Only the task role may use it, and
# only with Huxley's encryption context.
resource "aws_kms_key" "kek" {
  description             = "${var.name}: Huxley key-encryption key"
  key_usage               = "ENCRYPT_DECRYPT"
  enable_key_rotation     = true
  deletion_window_in_days = 30
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid       = "AccountAdministers"
        Effect    = "Allow"
        Principal = { AWS = "arn:${data.aws_partition.current.partition}:iam::${data.aws_caller_identity.current.account_id}:root" }
        Action    = "kms:*"
        Resource  = "*"
      },
      {
        Sid       = "HuxleyWrapsDataKeys"
        Effect    = "Allow"
        Principal = { AWS = aws_iam_role.task.arn }
        Action    = ["kms:Encrypt", "kms:Decrypt"]
        Resource  = "*"
        Condition = { StringEquals = { "kms:EncryptionContext:huxley" = "data-key" } }
      },
    ]
  })
}

resource "aws_kms_alias" "kek" {
  name          = "alias/${var.name}-kek"
  target_key_id = aws_kms_key.kek.key_id
}
