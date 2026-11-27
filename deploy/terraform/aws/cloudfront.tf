# Optional CloudFront in front of the load balancer (docs/operations.md,
# "Behind a CDN"). Nothing dynamic is cached; built assets under
# /_app/immutable/ are. The load balancer only answers requests carrying
# CloudFront's secret header, so nobody can go around it.

resource "random_password" "origin" {
  count   = var.cloudfront.enabled ? 1 : 0
  length  = 48
  special = false
}

data "aws_cloudfront_cache_policy" "disabled" {
  count = var.cloudfront.enabled ? 1 : 0
  name  = "Managed-CachingDisabled"
}

data "aws_cloudfront_cache_policy" "optimized" {
  count = var.cloudfront.enabled ? 1 : 0
  name  = "Managed-CachingOptimized"
}

data "aws_cloudfront_origin_request_policy" "all_viewer" {
  count = var.cloudfront.enabled ? 1 : 0
  name  = "Managed-AllViewer"
}

resource "aws_cloudfront_distribution" "main" {
  count           = var.cloudfront.enabled ? 1 : 0
  enabled         = true
  is_ipv6_enabled = true
  comment         = "Huxley (${var.name})"
  aliases         = [var.host, var.portal_host]
  price_class     = var.cloudfront.price_class
  http_version    = "http2and3"

  origin {
    origin_id   = "alb"
    domain_name = aws_lb.main.dns_name
    custom_origin_config {
      http_port                = 80
      https_port               = 443
      origin_protocol_policy   = "https-only"
      origin_ssl_protocols     = ["TLSv1.2"]
      origin_read_timeout      = 60
      origin_keepalive_timeout = 60
    }
    custom_header {
      name  = local.origin_header
      value = random_password.origin[0].result
    }
  }

  default_cache_behavior {
    target_origin_id         = "alb"
    viewer_protocol_policy   = "redirect-to-https"
    allowed_methods          = ["GET", "HEAD", "OPTIONS", "PUT", "POST", "PATCH", "DELETE"]
    cached_methods           = ["GET", "HEAD"]
    cache_policy_id          = data.aws_cloudfront_cache_policy.disabled[0].id
    origin_request_policy_id = data.aws_cloudfront_origin_request_policy.all_viewer[0].id
    compress                 = true
  }

  ordered_cache_behavior {
    path_pattern           = "/_app/immutable/*"
    target_origin_id       = "alb"
    viewer_protocol_policy = "redirect-to-https"
    allowed_methods        = ["GET", "HEAD"]
    cached_methods         = ["GET", "HEAD"]
    cache_policy_id        = data.aws_cloudfront_cache_policy.optimized[0].id
    compress               = true
  }

  viewer_certificate {
    acm_certificate_arn      = var.cloudfront.certificate_arn
    ssl_support_method       = "sni-only"
    minimum_protocol_version = "TLSv1.2_2021"
  }

  restrictions {
    geo_restriction {
      restriction_type = "none"
    }
  }

  lifecycle {
    precondition {
      condition     = var.cloudfront.certificate_arn != ""
      error_message = "cloudfront.certificate_arn (an ACM certificate in us-east-1 covering host and portal_host) is required with CloudFront."
    }
  }
}
