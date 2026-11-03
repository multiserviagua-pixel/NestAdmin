# Instala Ollama y descarga el modelo local de NestAdmin (D06) para desarrollo y evaluación.
# Reutiliza lo ya instalado y se puede repetir: `ollama pull` solo descarga lo que falta.
# El instalador final de la aplicación (R08) tendrá su propio flujo.
param([string]$Model = 'qwen3:8b')
$ErrorActionPreference = 'Stop'

$env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' +
    [Environment]::GetEnvironmentVariable('Path', 'User')

if (-not (Get-Command ollama -ErrorAction SilentlyContinue)) {
    Write-Host '==> Instalando Ollama'
    winget install --id Ollama.Ollama --exact --source winget --silent --accept-package-agreements --accept-source-agreements
    # 0 = instalado; -1978335189 = ya instalado y sin actualización aplicable.
    if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne -1978335189) {
        throw "winget falló al instalar Ollama (código $LASTEXITCODE)"
    }
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' +
        [Environment]::GetEnvironmentVariable('Path', 'User')
} else { Write-Host '==> Ollama ya instalado' }

# El servicio escucha solo en 127.0.0.1:11434; se arranca si no responde.
try { Invoke-RestMethod -Uri 'http://127.0.0.1:11434/api/version' -TimeoutSec 3 | Out-Null }
catch {
    Write-Host '==> Iniciando el servicio de Ollama'
    Start-Process ollama -ArgumentList 'serve' -WindowStyle Hidden
    Start-Sleep -Seconds 3
}

Write-Host "==> Descargando $Model (unos 5,2 GB la primera vez)"
ollama pull $Model
if ($LASTEXITCODE -ne 0) { throw "No se pudo descargar $Model" }

Write-Host '==> Comprobación'
ollama --version
ollama show $Model --modelfile | Select-String '^FROM' | Select-Object -First 1
(Invoke-RestMethod -Uri 'http://127.0.0.1:11434/api/tags').models |
    Where-Object { $_.name -eq $Model } |
    ForEach-Object { "Modelo: $($_.name)  digest: $($_.digest)  tamaño: $([math]::Round($_.size / 1GB, 2)) GB" }
