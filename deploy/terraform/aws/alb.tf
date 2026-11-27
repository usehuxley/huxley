# The load balancer routes as the Compose Caddyfile does: the API, webhooks,
# SCIM, MCP and its OAuth endpoints to the API; the rest of the staff host to
# the web app; the portal host to the portal.

resource "aws_lb" "main" {
  name                       = var.name
  load_balancer_type         = "application"
  internal                   = false
  subnets                    = aws_subnet.public[*].id
  security_groups            = [aws_security_group.alb.id]
  drop_invalid_header_fields = true
  enable_deletion_protection = var.deletion_protection
  # Synchronous webhooks can wait up to 5 minutes for a run.
  idle_timeout = 310
}

locals {
  targets = {
    api    = { port = 8080, health = "/readyz" }
    web    = { port = 3000, health = "/healthz" }
    portal = { port = 3000, health = "/healthz" }
  }
  # Three paths per rule: a rule allows five match evaluations, and the host
  # (and CloudFront's header) take the others.
  api_paths = [
    ["/api/*", "/hooks/*", "/scim/*"],
    ["/callbacks/*", "/mcp", "/mcp/*"],
    ["/.well-known/oauth-*", "/oauth/register", "/oauth/token"],
    ["/oauth/revoke"],
  ]
  origin_header = "X-Huxley-Origin-Verify"
}

resource "aws_lb_target_group" "app" {
  for_each             = local.targets
  name                 = "${var.name}-${each.key}"
  port                 = each.value.port
  protocol             = "HTTP"
  target_type          = "ip"
  vpc_id               = aws_vpc.main.id
  deregistration_delay = 30
  health_check {
    path                = each.value.health
    matcher             = "200"
    interval            = 15
    healthy_threshold   = 2
    unhealthy_threshold = 3
  }
}

resource "aws_lb_listener" "http" {
  load_balancer_arn = aws_lb.main.arn
  port              = 80
  protocol          = "HTTP"
  default_action {
    type = "redirect"
    redirect {
      port        = "443"
      protocol    = "HTTPS"
      status_code = "HTTP_301"
    }
  }
}

resource "aws_lb_listener" "https" {
  load_balancer_arn = aws_lb.main.arn
  port              = 443
  protocol          = "HTTPS"
  ssl_policy        = "ELBSecurityPolicy-TLS13-1-2-2021-06"
  certificate_arn   = var.certificate_arn
  default_action {
    type = "fixed-response"
    fixed_response {
      content_type = "text/plain"
      message_body = "Not found"
      status_code  = "404"
    }
  }
}

resource "aws_lb_listener_rule" "api" {
  count        = length(local.api_paths)
  listener_arn = aws_lb_listener.https.arn
  priority     = 10 + count.index
  action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.app["api"].arn
  }
  condition {
    host_header { values = [var.host] }
  }
  condition {
    path_pattern { values = local.api_paths[count.index] }
  }
  dynamic "condition" {
    for_each = var.cloudfront.enabled ? [1] : []
    content {
      http_header {
        http_header_name = local.origin_header
        values           = [random_password.origin[0].result]
      }
    }
  }
}

resource "aws_lb_listener_rule" "frontend" {
  for_each     = { web = { host = var.host, priority = 100 }, portal = { host = var.portal_host, priority = 101 } }
  listener_arn = aws_lb_listener.https.arn
  priority     = each.value.priority
  action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.app[each.key].arn
  }
  condition {
    host_header { values = [each.value.host] }
  }
  dynamic "condition" {
    for_each = var.cloudfront.enabled ? [1] : []
    content {
      http_header {
        http_header_name = local.origin_header
        values           = [random_password.origin[0].result]
      }
    }
  }
}
