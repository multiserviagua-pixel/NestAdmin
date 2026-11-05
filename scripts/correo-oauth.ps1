# Genera correo-oauth.json (D43, D50) con los clientes OAuth del editor, FUERA del repositorio:
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/correo-oauth.ps1 `
#       -Google C:\fuera\client_secret_XXXX.json -MicrosoftClientId 00000000-0000-0000-0000-000000000000 `
#       -Salida C:\fuera\correo-oauth.json
#
# -Google es el JSON que descarga Google Cloud para un cliente de tipo «App de escritorio»
# (bloque "installed"). -MicrosoftClientId es el «Id. de aplicación (cliente)» del registro en
# Entra ID. Basta con uno de los dos. No muestra los valores en pantalla.
param(
    [string]$Google = '',
    [string]$MicrosoftClientId = '',
    [Parameter(Mandatory = $true)][string]$Salida,
    [switch]$Sobrescribir
)
$ErrorActionPreference = 'Stop'

if (-not $Google -and -not $MicrosoftClientId) { throw 'Indique -Google, -MicrosoftClientId o ambos' }

$repo = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot)).TrimEnd('\') + '\'
$Salida = [IO.Path]::GetFullPath($Salida)
if ($Salida.StartsWith($repo, [StringComparison]::OrdinalIgnoreCase)) {
    throw "La salida debe quedar fuera del repositorio ($repo): contiene el secreto del cliente de Google"
}
if ((Test-Path -LiteralPath $Salida) -and -not $Sobrescribir) { throw "Ya existe $Salida (use -Sobrescribir)" }

$result = [ordered]@{}
if ($Google) {
    $downloaded = Get-Content -LiteralPath $Google -Raw -Encoding UTF8 | ConvertFrom-Json
    if (-not $downloaded.installed) {
        if ($downloaded.web) { throw 'El JSON de Google es de un cliente «Aplicación web»; cree uno de tipo «App de escritorio»' }
        throw 'El JSON de Google no tiene el bloque "installed"'
    }
    $id = "$($downloaded.installed.client_id)".Trim()
    $secret = "$($downloaded.installed.client_secret)".Trim()
    if (-not $id.EndsWith('.apps.googleusercontent.com')) { throw 'installed.client_id no parece un ID de cliente de Google' }
    if (-not $secret) { throw 'Falta installed.client_secret' }
    $result.google = [ordered]@{ client_id = $id; client_secret = $secret }
}
if ($MicrosoftClientId) {
    $guid = [guid]::Empty
    if (-not [guid]::TryParse($MicrosoftClientId.Trim(), [ref]$guid) -or $guid -eq [guid]::Empty) {
        throw '-MicrosoftClientId debe ser el GUID del «Id. de aplicación (cliente)» de Entra ID'
    }
    $result.microsoft = [ordered]@{ client_id = $guid.ToString() }
}

$folder = Split-Path -Parent $Salida
if (-not (Test-Path -LiteralPath $folder)) { New-Item -ItemType Directory -Path $folder | Out-Null }
[IO.File]::WriteAllText($Salida, ($result | ConvertTo-Json -Depth 3))   # UTF-8 sin BOM
Write-Host "Escrito $Salida con: $(@($result.Keys) -join ', ')."
Write-Host 'Constrúyalo en el instalador con: scripts/build-clean.ps1 -ClienteOAuth <esa ruta>'
