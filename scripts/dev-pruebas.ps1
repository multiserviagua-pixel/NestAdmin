# Abre la versión de desarrollo (la rama actual, normalmente main) sin instalar nada y con una
# carpeta de datos aparte, para no tocar ni migrar la de la versión instalada.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\dev-pruebas.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\dev-pruebas.ps1 -Datos D:\otra-carpeta
#
# - Datos: %USERPROFILE%\NestAdmin-pruebas por defecto (fuera del repositorio). Si no tiene
#   correo-oauth.json, copia el de la instalación (%LOCALAPPDATA%\NestAdmin) cuando existe.
# - Las credenciales (Administrador de credenciales, servicio NestAdmin) son las mismas que usa
#   la versión instalada.
# - Solo puede haber una instancia: si la versión instalada está abierta, ciérrela antes desde el
#   icono del área de notificación («Salir de NestAdmin por completo»).
# - Compila sin optimizar: los tiempos y la memoria no valen para T12 (use scripts\t12-medir.ps1).
param([string]$Datos = (Join-Path $env:USERPROFILE 'NestAdmin-pruebas'))
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot

if (Get-Process nestadmin -ErrorAction SilentlyContinue) {
    throw 'NestAdmin ya está abierto. Ciérrelo desde el área de notificación («Salir de NestAdmin por completo») y vuelva a ejecutar.'
}
New-Item -ItemType Directory -Force $Datos | Out-Null
$oauth = Join-Path $Datos 'correo-oauth.json'
$instalado = Join-Path $env:LOCALAPPDATA 'NestAdmin\correo-oauth.json'
if (-not (Test-Path $oauth) -and (Test-Path $instalado)) {
    Copy-Item $instalado $oauth
    Write-Host "Copiado correo-oauth.json de la instalación a $Datos"
}
if (-not (Test-Path (Join-Path $Root 'ui\node_modules'))) {
    npm ci --prefix (Join-Path $Root 'ui')
    if ($LASTEXITCODE -ne 0) { throw 'npm ci falló' }
}
$env:NESTADMIN_DATA_DIR = $Datos
Write-Host "Datos de prueba: $Datos"
Set-Location (Join-Path $Root 'crates\desktop')
& (Join-Path $Root 'ui\node_modules\.bin\tauri.cmd') dev
