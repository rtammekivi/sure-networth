{{- define "sure-networth.name" -}}
{{- default "sure-networth" .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "sure-networth.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default "sure-networth" .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{- define "sure-networth.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "sure-networth.labels" -}}
helm.sh/chart: {{ include "sure-networth.chart" . }}
{{ include "sure-networth.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{- define "sure-networth.selectorLabels" -}}
app.kubernetes.io/name: {{ include "sure-networth.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{- define "sure-networth.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "sure-networth.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{- define "sure-networth.configMapName" -}}
{{- default (include "sure-networth.fullname" .) .Values.mapping.existingConfigMap }}
{{- end }}

{{- define "sure-networth.publicUrl" -}}
{{- if and .Values.ingress.enabled .Values.ingress.hosts }}
{{- $host := (first .Values.ingress.hosts).host }}
{{- $scheme := ternary "https" "http" (not (empty .Values.ingress.tls)) }}
{{- printf "%s://%s" $scheme $host }}
{{- else if and .Values.httpRoute.enabled .Values.httpRoute.hostnames }}
{{- printf "https://%s" (first .Values.httpRoute.hostnames) }}
{{- end }}
{{- end }}
