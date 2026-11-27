output "dns" {
  description = "Point these names at this target (CNAME, or an alias record in Route 53)."
  value = {
    names  = [var.host, var.portal_host]
    target = var.cloudfront.enabled ? aws_cloudfront_distribution.main[0].domain_name : aws_lb.main.dns_name
  }
}

output "bootstrap" {
  description = "Set the instance up once, after the API is healthy: it prints the first user's password."
  value       = "aws ecs execute-command --region ${var.region} --cluster ${aws_ecs_cluster.main.name} --container huxley --interactive --task $(aws ecs list-tasks --region ${var.region} --cluster ${aws_ecs_cluster.main.name} --service-name api --query 'taskArns[0]' --output text) --command 'huxley admin bootstrap --org-name \"Your Company\" --email you@example.com --name \"Your Name\" --preset service-desk'"
}

output "kms_key_arn" {
  description = "The key-encryption key. Huxley's data keys are useless without it: don't delete it."
  value       = aws_kms_key.kek.arn
}

output "objects_bucket" {
  value = aws_s3_bucket.objects.bucket
}

output "database_endpoint" {
  value = aws_db_instance.main.address
}

output "logs" {
  value = aws_cloudwatch_log_group.main.name
}
