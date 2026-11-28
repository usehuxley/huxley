{{/* Names and labels. */}}
{{- define "huxley.fullname" -}}
{{- if contains .Chart.Name .Release.Name -}}
{{- .Release.Name | trunc 50 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name .Chart.Name | trunc 50 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}

{{- define "huxley.labels" -}}
app.kubernetes.io/name: huxley
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version }}
{{- end -}}

{{/* Selector labels for one component: (dict "ctx" $ "component" "api"). */}}
{{- define "huxley.selector" -}}
app.kubernetes.io/name: huxley
app.kubernetes.io/instance: {{ .ctx.Release.Name }}
app.kubernetes.io/component: {{ .component }}
{{- end -}}

{{- define "huxley.serviceAccount" -}}
{{- if .Values.serviceAccount.create -}}
{{- default (include "huxley.fullname" .) .Values.serviceAccount.name -}}
{{- else -}}
{{- default "default" .Values.serviceAccount.name -}}
{{- end -}}
{{- end -}}

{{/* An image: (dict "ctx" $ "name" "huxley"). */}}
{{- define "huxley.image" -}}
{{- printf "%s/%s:%s" .ctx.Values.image.registry .name (default .ctx.Chart.AppVersion .ctx.Values.image.tag) -}}
{{- end -}}

{{- define "huxley.dbSecret" -}}
{{- default (printf "%s-database" (include "huxley.fullname" .)) .Values.database.existingSecret -}}
{{- end -}}

{{- define "huxley.kekSecret" -}}
{{- default (printf "%s-kek" (include "huxley.fullname" .)) .Values.kek.existingSecret -}}
{{- end -}}

{{/* postgres:// URL to a host, with the password from $(DB_PASSWORD). */}}
{{- define "huxley.dbUrl" -}}
{{- $db := .ctx.Values.database -}}
{{- printf "postgres://%s:$(DB_PASSWORD)@%s:%v/%s?sslmode=%s" $db.user .host .port $db.name .sslmode -}}
{{- end -}}

{{- define "huxley.validate" -}}
{{- if not .Values.database.host -}}
{{- fail "database.host is required" -}}
{{- end -}}
{{- if and (not .Values.database.existingSecret) (not .Values.database.password) -}}
{{- fail "set database.existingSecret (or database.password)" -}}
{{- end -}}
{{- if and (not .Values.kek.existingSecret) (not .Values.kek.value) -}}
{{- fail "set kek.existingSecret (or kek.value)" -}}
{{- end -}}
{{- $scan := include "huxley.scanProvider" . -}}
{{- if not (has $scan (list "off" "clamav" "metadefender")) -}}
{{- fail "scan.provider must be \"off\", \"clamav\" or \"metadefender\"" -}}
{{- end -}}
{{- if and (eq $scan "clamav") (not .Values.scan.clamav.deploy) (not .Values.scan.clamav.address) -}}
{{- fail "scan.provider is clamav: set scan.clamav.deploy: true, or scan.clamav.address to your clamd" -}}
{{- end -}}
{{- if and (eq $scan "metadefender") (contains "metadefender.com" .Values.scan.metadefender.url) (not .Values.scan.metadefender.apiKeySecret) -}}
{{- fail "MetaDefender Cloud needs an API key: set scan.metadefender.apiKeySecret" -}}
{{- end -}}
{{- if and (not .Values.storage.url) (not .Values.storage.persistence.enabled) -}}
{{- fail "set storage.url (s3:// or az://) or storage.persistence.enabled: several pods share attachments" -}}
{{- end -}}
{{- end -}}

{{/* scan.provider as text (a bare off in YAML is false). */}}
{{- define "huxley.scanProvider" -}}
{{- $p := toString .Values.scan.provider -}}
{{- if eq $p "false" -}}off{{- else -}}{{ $p }}{{- end -}}
{{- end -}}

{{/* The environment every Huxley process shares. */}}
{{- define "huxley.env" -}}
- name: DB_PASSWORD
  valueFrom:
    secretKeyRef: { name: {{ include "huxley.dbSecret" . }}, key: {{ .Values.database.passwordKey }} }
{{- if .Values.pgbouncer.enabled }}
- name: HUXLEY_DATABASE__URL
  value: {{ include "huxley.dbUrl" (dict "ctx" . "host" (printf "%s-pgbouncer" (include "huxley.fullname" .)) "port" 6432 "sslmode" "disable") | quote }}
- name: HUXLEY_DATABASE__POOLED
  value: "true"
- name: HUXLEY_DATABASE__DIRECT_URL
  value: {{ include "huxley.dbUrl" (dict "ctx" . "host" .Values.database.host "port" .Values.database.port "sslmode" .Values.database.sslmode) | quote }}
{{- else }}
- name: HUXLEY_DATABASE__URL
  value: {{ include "huxley.dbUrl" (dict "ctx" . "host" .Values.database.host "port" .Values.database.port "sslmode" .Values.database.sslmode) | quote }}
{{- end }}
{{- with .Values.database.readReplicaHost }}
- name: HUXLEY_DATABASE__READ_URL
  value: {{ include "huxley.dbUrl" (dict "ctx" $ "host" . "port" $.Values.database.port "sslmode" $.Values.database.sslmode) | quote }}
{{- end }}
- name: HUXLEY_DATABASE__MAX_CONNECTIONS
  value: {{ .Values.database.maxConnections | quote }}
- name: HUXLEY_DATABASE__STATEMENT_TIMEOUT_MS
  value: {{ .Values.database.statementTimeoutMs | quote }}
- name: HUXLEY_CRYPTO__KEK
  valueFrom:
    secretKeyRef: { name: {{ include "huxley.kekSecret" . }}, key: {{ .Values.kek.key }} }
- name: HUXLEY_TELEMETRY__LOG_FORMAT
  value: json
- name: HUXLEY_TELEMETRY__ENVIRONMENT
  value: {{ .Values.telemetry.environment | quote }}
{{- with .Values.telemetry.otlpEndpoint }}
- name: HUXLEY_TELEMETRY__OTLP_ENDPOINT
  value: {{ . | quote }}
{{- end }}
- name: HUXLEY_HTTP__TRUST_FORWARDED_FOR
  value: "true"
- name: HUXLEY_HTTP__PROXY_HOPS
  value: {{ .Values.clientIp.proxyHops | quote }}
{{- with .Values.clientIp.header }}
- name: HUXLEY_HTTP__CLIENT_IP_HEADER
  value: {{ . | quote }}
{{- end }}
- name: HUXLEY_AUTH__PUBLIC_URL
  value: {{ printf "%s://%s" .Values.scheme .Values.host | quote }}
- name: HUXLEY_HTTP__PORTAL_PUBLIC_URL
  value: {{ printf "%s://%s" .Values.scheme .Values.portalHost | quote }}
{{- if .Values.storage.persistence.enabled }}
- name: HUXLEY_STORAGE__URL
  value: /var/lib/huxley/objects
{{- else }}
- name: HUXLEY_STORAGE__URL
  value: {{ .Values.storage.url | quote }}
{{- end }}
- name: HUXLEY_STORAGE__MAX_UPLOAD_MB
  value: {{ .Values.storage.maxUploadMb | quote }}
- name: HUXLEY_EMAIL__FROM
  value: {{ .Values.email.from | quote }}
{{- with .Values.email.smtpUrlSecret }}
- name: HUXLEY_EMAIL__SMTP_URL
  valueFrom:
    secretKeyRef: { name: {{ . }}, key: {{ $.Values.email.smtpUrlKey }} }
{{- end }}
{{- $scan := include "huxley.scanProvider" . }}
- name: HUXLEY_UPLOADS__SCAN__PROVIDER
  value: {{ $scan | quote }}
{{- if ne $scan "off" }}
- name: HUXLEY_UPLOADS__SCAN__WHILE_PENDING
  value: {{ .Values.scan.whilePending | quote }}
{{- end }}
{{- if eq $scan "clamav" }}
- name: HUXLEY_UPLOADS__SCAN__CLAMAV__ADDRESS
  value: {{ .Values.scan.clamav.address | default (printf "tcp://%s-clamav:3310" (include "huxley.fullname" .)) | quote }}
{{- end }}
{{- if eq $scan "metadefender" }}
- name: HUXLEY_UPLOADS__SCAN__METADEFENDER__URL
  value: {{ .Values.scan.metadefender.url | quote }}
{{- with .Values.scan.metadefender.rule }}
- name: HUXLEY_UPLOADS__SCAN__METADEFENDER__RULE
  value: {{ . | quote }}
{{- end }}
{{- with .Values.scan.metadefender.apiKeySecret }}
- name: HUXLEY_UPLOADS__SCAN__METADEFENDER__API_KEY
  valueFrom:
    secretKeyRef: { name: {{ . }}, key: {{ $.Values.scan.metadefender.apiKeyKey }} }
{{- end }}
{{- end }}
{{- if .Values.api.runnerGateway }}
- name: HUXLEY_RUNNERS__PUBLIC_URL
  value: {{ printf "https://%s-api:8443" (include "huxley.fullname" .) | quote }}
- name: HUXLEY_RUNNERS__HOSTS
  value: {{ printf "[%s-api]" (include "huxley.fullname" .) | quote }}
{{- end }}
{{- with .Values.extraEnv }}
{{ toYaml . }}
{{- end }}
{{- end -}}

{{- define "huxley.envFrom" -}}
{{- with .Values.storage.optionsSecret }}
- secretRef: { name: {{ . }} }
{{- end }}
{{- with .Values.extraEnvFrom }}
{{ toYaml . }}
{{- end }}
{{- end -}}

{{/* Volumes for a Huxley process: a writable /tmp, and shared objects. */}}
{{- define "huxley.volumes" -}}
- name: tmp
  emptyDir: {}
{{- if .Values.storage.persistence.enabled }}
- name: objects
  persistentVolumeClaim:
    claimName: {{ default (printf "%s-objects" (include "huxley.fullname" .)) .Values.storage.persistence.existingClaim }}
{{- end }}
{{- end -}}

{{- define "huxley.volumeMounts" -}}
- name: tmp
  mountPath: /tmp
{{- if .Values.storage.persistence.enabled }}
- name: objects
  mountPath: /var/lib/huxley/objects
{{- end }}
{{- end -}}

{{/* A Huxley server Deployment: (dict "ctx" $ "component" "api" "roles" "api" "replicas" 2 "resources" {...} "env" [...] "ports" [...] "probes" bool "scheduling" {...}). */}}
{{- define "huxley.serverDeployment" -}}
{{- $ := .ctx -}}
apiVersion: apps/v1
kind: Deployment
metadata:
  name: {{ include "huxley.fullname" $ }}-{{ .component }}
  labels:
    {{- include "huxley.labels" $ | nindent 4 }}
    app.kubernetes.io/component: {{ .component }}
spec:
  {{- if not .autoscaled }}
  replicas: {{ .replicas }}
  {{- end }}
  revisionHistoryLimit: 5
  selector:
    matchLabels:
      {{- include "huxley.selector" (dict "ctx" $ "component" .component) | nindent 6 }}
  template:
    metadata:
      labels:
        {{- include "huxley.selector" (dict "ctx" $ "component" .component) | nindent 8 }}
      annotations:
        checksum/config: {{ include "huxley.env" $ | sha256sum | trunc 16 }}
    spec:
      serviceAccountName: {{ include "huxley.serviceAccount" $ }}
      {{- with $.Values.imagePullSecrets }}
      imagePullSecrets: {{ toYaml . | nindent 8 }}
      {{- end }}
      securityContext: {{ toYaml $.Values.podSecurityContext | nindent 8 }}
      terminationGracePeriodSeconds: 60
      containers:
        - name: huxley
          image: {{ include "huxley.image" (dict "ctx" $ "name" "huxley") }}
          imagePullPolicy: {{ $.Values.image.pullPolicy }}
          args: ["serve", "--roles", {{ .roles | quote }}]
          securityContext: {{ toYaml $.Values.securityContext | nindent 12 }}
          env:
            {{- include "huxley.env" $ | nindent 12 }}
            {{- with .env }}
            {{- toYaml . | nindent 12 }}
            {{- end }}
          {{- with (include "huxley.envFrom" $) }}
          envFrom: {{ . | nindent 12 }}
          {{- end }}
          {{- with .ports }}
          ports: {{ toYaml . | nindent 12 }}
          {{- end }}
          {{- if .probes }}
          startupProbe:
            httpGet: { path: /healthz, port: http }
            periodSeconds: 2
            failureThreshold: 60
          livenessProbe:
            httpGet: { path: /healthz, port: http }
            periodSeconds: 15
          readinessProbe:
            httpGet: { path: /readyz, port: http }
            periodSeconds: 5
          {{- end }}
          resources: {{ toYaml .resources | nindent 12 }}
          volumeMounts: {{ include "huxley.volumeMounts" $ | nindent 12 }}
      volumes: {{ include "huxley.volumes" $ | nindent 8 }}
      {{- with .scheduling }}
      {{- with .nodeSelector }}
      nodeSelector: {{ toYaml . | nindent 8 }}
      {{- end }}
      {{- with .tolerations }}
      tolerations: {{ toYaml . | nindent 8 }}
      {{- end }}
      {{- with .affinity }}
      affinity: {{ toYaml . | nindent 8 }}
      {{- end }}
      {{- end }}
{{- end -}}

{{- define "huxley.pdb" -}}
{{- if .ctx.Values.podDisruptionBudgets }}
---
apiVersion: policy/v1
kind: PodDisruptionBudget
metadata:
  name: {{ include "huxley.fullname" .ctx }}-{{ .component }}
  labels:
    {{- include "huxley.labels" .ctx | nindent 4 }}
spec:
  maxUnavailable: 1
  selector:
    matchLabels:
      {{- include "huxley.selector" (dict "ctx" .ctx "component" .component) | nindent 6 }}
{{- end }}
{{- end -}}
