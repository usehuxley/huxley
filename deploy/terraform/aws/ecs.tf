# ECS Fargate: the API, worker pools by job class, the scheduler, the web
# app and the portal. Migrations run at start (concurrent starts take turns).

locals {
  db_url = "postgres://huxley@${aws_db_instance.main.address}:5432/huxley?sslmode=require"
  image  = { for app in ["huxley", "web", "portal"] : app => "${var.image_registry}/${app}:${var.image_tag}" }
  # Behind CloudFront, the client is two proxies back (CloudFront, then the
  # load balancer); behind the load balancer alone, one.
  proxy_hops = var.cloudfront.enabled ? 2 : 1

  huxley_env = concat([
    { name = "HUXLEY_DATABASE__URL", value = local.db_url },
    { name = "HUXLEY_MIGRATE_ON_START", value = "true" },
    { name = "HUXLEY_CRYPTO__AWS_KMS__KEY_ID", value = aws_kms_alias.kek.arn },
    { name = "HUXLEY_CRYPTO__AWS_KMS__REGION", value = var.region },
    { name = "HUXLEY_STORAGE__URL", value = "s3://${aws_s3_bucket.objects.bucket}" },
    { name = "HUXLEY_STORAGE__OPTIONS__AWS_REGION", value = var.region },
    { name = "HUXLEY_HTTP__TRUST_FORWARDED_FOR", value = "true" },
    { name = "HUXLEY_HTTP__PROXY_HOPS", value = tostring(local.proxy_hops) },
    { name = "HUXLEY_AUTH__PUBLIC_URL", value = "https://${var.host}" },
    { name = "HUXLEY_HTTP__PORTAL_PUBLIC_URL", value = "https://${var.portal_host}" },
    { name = "HUXLEY_EMAIL__FROM", value = var.email_from },
    { name = "HUXLEY_TELEMETRY__LOG_FORMAT", value = "json" },
    { name = "HUXLEY_TELEMETRY__ENVIRONMENT", value = var.name },
    ], var.otlp_endpoint == "" ? [] : [
    { name = "HUXLEY_TELEMETRY__OTLP_ENDPOINT", value = var.otlp_endpoint },
  ])

  # The database password, from the secret RDS manages (sqlx reads
  # PGPASSWORD when the URL has none).
  huxley_secrets = concat([
    { name = "PGPASSWORD", valueFrom = "${aws_db_instance.main.master_user_secret[0].secret_arn}:password::" },
    ], var.smtp_url_secret_arn == "" ? [] : [
    { name = "HUXLEY_EMAIL__SMTP_URL", valueFrom = var.smtp_url_secret_arn },
  ])

  servers = merge(
    {
      api = {
        roles = "api", count = var.api.count, cpu = var.api.cpu, memory = var.api.memory, port = 8080, env = []
      }
      scheduler = {
        roles = "scheduler", count = var.scheduler.count, cpu = var.scheduler.cpu, memory = var.scheduler.memory, port = null, env = []
      }
    },
    { for name, pool in var.worker_pools : "worker-${name}" => {
      roles = "worker", count = pool.count, cpu = pool.cpu, memory = pool.memory, port = null
      env = concat(
        [{ name = "HUXLEY_ENGINE__CONCURRENCY", value = tostring(pool.concurrency) }],
        length(pool.kinds) == 0 ? [] : [{ name = "HUXLEY_ENGINE__KINDS", value = "[${join(",", pool.kinds)}]" }],
      )
    } },
  )

  frontends = {
    web    = { host = var.host, extra = [{ name = "BODY_SIZE_LIMIT", value = "30M" }] }
    portal = { host = var.portal_host, extra = [] }
  }
}

resource "aws_cloudwatch_log_group" "main" {
  name              = "/huxley/${var.name}"
  retention_in_days = var.log_retention_days
}

resource "aws_ecs_cluster" "main" {
  name = var.name
  setting {
    name  = "containerInsights"
    value = "enhanced"
  }
}

resource "aws_service_discovery_http_namespace" "main" {
  name        = var.name
  description = "Huxley services (ECS Service Connect)"
}

# --- IAM -------------------------------------------------------------------

data "aws_iam_policy_document" "ecs_assume" {
  statement {
    actions = ["sts:AssumeRole"]
    principals {
      type        = "Service"
      identifiers = ["ecs-tasks.amazonaws.com"]
    }
  }
}

# What ECS needs to start tasks: pull images, write logs, read secrets.
resource "aws_iam_role" "execution" {
  name               = "${var.name}-execution"
  assume_role_policy = data.aws_iam_policy_document.ecs_assume.json
}

resource "aws_iam_role_policy_attachment" "execution" {
  role       = aws_iam_role.execution.name
  policy_arn = "arn:${data.aws_partition.current.partition}:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

resource "aws_iam_role_policy" "execution_secrets" {
  name = "secrets"
  role = aws_iam_role.execution.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect   = "Allow"
      Action   = ["secretsmanager:GetSecretValue"]
      Resource = compact([aws_db_instance.main.master_user_secret[0].secret_arn, var.smtp_url_secret_arn])
    }]
  })
}

# What Huxley itself may do: its bucket, its key (see the key policy), and
# ECS Exec for the bootstrap command.
resource "aws_iam_role" "task" {
  name               = "${var.name}-task"
  assume_role_policy = data.aws_iam_policy_document.ecs_assume.json
}

resource "aws_iam_role_policy" "task" {
  name = "huxley"
  role = aws_iam_role.task.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect   = "Allow"
        Action   = ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"]
        Resource = "${aws_s3_bucket.objects.arn}/*"
      },
      {
        Effect   = "Allow"
        Action   = ["s3:ListBucket"]
        Resource = aws_s3_bucket.objects.arn
      },
      {
        Effect    = "Allow"
        Action    = ["kms:Encrypt", "kms:Decrypt"]
        Resource  = aws_kms_key.kek.arn
        Condition = { StringEquals = { "kms:EncryptionContext:huxley" = "data-key" } }
      },
      {
        Effect = "Allow"
        Action = [
          "ssmmessages:CreateControlChannel", "ssmmessages:CreateDataChannel",
          "ssmmessages:OpenControlChannel", "ssmmessages:OpenDataChannel",
        ]
        Resource = "*"
      },
    ]
  })
}

# --- Huxley server tasks ---------------------------------------------------

resource "aws_ecs_task_definition" "server" {
  for_each                 = local.servers
  family                   = "${var.name}-${each.key}"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = each.value.cpu
  memory                   = each.value.memory
  execution_role_arn       = aws_iam_role.execution.arn
  task_role_arn            = aws_iam_role.task.arn
  runtime_platform {
    operating_system_family = "LINUX"
    cpu_architecture        = "X86_64"
  }
  container_definitions = jsonencode([{
    name                   = "huxley"
    image                  = local.image.huxley
    essential              = true
    command                = ["serve", "--roles", each.value.roles]
    environment            = concat(local.huxley_env, each.value.env)
    secrets                = local.huxley_secrets
    portMappings           = each.value.port == null ? [] : [{ name = "http", containerPort = each.value.port, protocol = "tcp" }]
    readonlyRootFilesystem = true
    mountPoints            = [{ sourceVolume = "tmp", containerPath = "/tmp" }]
    stopTimeout            = 60
    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = aws_cloudwatch_log_group.main.name
        "awslogs-region"        = var.region
        "awslogs-stream-prefix" = each.key
      }
    }
  }])
  volume {
    name = "tmp"
  }
}

resource "aws_ecs_service" "server" {
  for_each               = local.servers
  name                   = each.key
  cluster                = aws_ecs_cluster.main.id
  task_definition        = aws_ecs_task_definition.server[each.key].arn
  desired_count          = each.value.count
  launch_type            = "FARGATE"
  enable_execute_command = each.key == "api"
  propagate_tags         = "SERVICE"
  network_configuration {
    subnets         = aws_subnet.private[*].id
    security_groups = [aws_security_group.tasks.id]
  }
  deployment_circuit_breaker {
    enable   = true
    rollback = true
  }
  # The API answers the web apps at http://api:8080. Plain TCP, so the
  # proxy leaves X-Forwarded-For alone. Workers and the scheduler don't
  # need it (and skip its sidecar).
  service_connect_configuration {
    enabled   = each.key == "api"
    namespace = aws_service_discovery_http_namespace.main.arn
    dynamic "service" {
      for_each = each.key == "api" ? [1] : []
      content {
        port_name = "http"
        client_alias {
          dns_name = "api"
          port     = 8080
        }
      }
    }
  }
  dynamic "load_balancer" {
    for_each = each.key == "api" ? [1] : []
    content {
      target_group_arn = aws_lb_target_group.app["api"].arn
      container_name   = "huxley"
      container_port   = 8080
    }
  }
  depends_on = [aws_lb_listener.https]
}

# --- Web app and portal ----------------------------------------------------

resource "aws_ecs_task_definition" "frontend" {
  for_each                 = local.frontends
  family                   = "${var.name}-${each.key}"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = var.frontend.cpu
  memory                   = var.frontend.memory
  execution_role_arn       = aws_iam_role.execution.arn
  runtime_platform {
    operating_system_family = "LINUX"
    cpu_architecture        = "X86_64"
  }
  container_definitions = jsonencode([{
    name      = each.key
    image     = local.image[each.key]
    essential = true
    environment = concat([
      { name = "HUXLEY_API_URL", value = "http://api:8080" },
      { name = "ORIGIN", value = "https://${each.value.host}" },
      { name = "ADDRESS_HEADER", value = "X-Forwarded-For" },
      { name = "XFF_DEPTH", value = tostring(local.proxy_hops) },
    ], each.value.extra)
    portMappings           = [{ name = "http", containerPort = 3000, protocol = "tcp" }]
    readonlyRootFilesystem = true
    mountPoints            = [{ sourceVolume = "tmp", containerPath = "/tmp" }]
    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = aws_cloudwatch_log_group.main.name
        "awslogs-region"        = var.region
        "awslogs-stream-prefix" = each.key
      }
    }
  }])
  volume {
    name = "tmp"
  }
}

resource "aws_ecs_service" "frontend" {
  for_each        = local.frontends
  name            = each.key
  cluster         = aws_ecs_cluster.main.id
  task_definition = aws_ecs_task_definition.frontend[each.key].arn
  desired_count   = var.frontend.count
  launch_type     = "FARGATE"
  propagate_tags  = "SERVICE"
  network_configuration {
    subnets         = aws_subnet.private[*].id
    security_groups = [aws_security_group.tasks.id]
  }
  deployment_circuit_breaker {
    enable   = true
    rollback = true
  }
  service_connect_configuration {
    enabled   = true
    namespace = aws_service_discovery_http_namespace.main.arn
  }
  load_balancer {
    target_group_arn = aws_lb_target_group.app[each.key].arn
    container_name   = each.key
    container_port   = 3000
  }
  depends_on = [aws_lb_listener.https]
}
