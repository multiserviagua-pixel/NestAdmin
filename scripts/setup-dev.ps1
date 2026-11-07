# Instala o comprueba en Windows las herramientas de desarrollo de NestAdmin con las versiones
# fijadas (decisiones.md, «Versiones de desarrollo fijadas»; D60). Reutiliza lo ya instalado
# en la versión correcta y se puede repetir sin efectos duplicados.
#
#   -Comprobar   solo informa, sin instalar ni cambiar nada; sale con 1 si falta algo.
#
# Solo pide las confirmaciones del propio Windows (UAC de los instaladores de Python, Build
# Tools y WebView2). No instala Ollama, modelos ni OCR (scripts/setup-ai.ps1 y setup-ocr.ps1).
# NSIS no hace falta: tauri-cli descarga y comprueba la versión que necesita al empaquetar.
param([switch]$Comprobar)
$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
# Versiones fijadas. Node sale de .node-version y Rust de rust-toolchain.toml, para que CI,
# este script y build-clean.ps1 usen las mismas.
$NodeVersion = (Get-Content (Join-Path $Root '.node-version') -TotalCount 1).Trim()
$RustVersion = ((Get-Content (Join-Path $Root 'rust-toolchain.toml')) -match '^channel' -replace '.*"(.+)".*', '$1')
$PythonSeries = '3.12'
$VcTools = 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64'
# Cualquiera de estos SDK de Windows sirve; se instala el primero si no hay ninguno.
$WindowsSdks = @(
    'Microsoft.VisualStudio.Component.Windows11SDK.22621',
    'Microsoft.VisualStudio.Component.Windows11SDK.26100'
)
$WebView2Id = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'

function Update-SessionPath {
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' +
        [Environment]::GetEnvironmentVariable('Path', 'User') + ';' +
        (Join-Path $env:USERPROFILE '.cargo\bin')
}

# Ejecuta una herramienta y devuelve su primera línea, o $null si no existe.
function Get-ToolLine([string]$Name, [string[]]$Arguments) {
    $cmd = Get-Command $Name -ErrorAction SilentlyContinue
    # El alias de la Microsoft Store para python no es un intérprete real.
    if (-not $cmd -or $cmd.Source -match '\\WindowsApps\\') { return $null }
    # Solo la salida estándar: en PowerShell 5.1 el texto de stderr (rustup escribe ahí sus
    # avisos) se convierte en error y, con 'Stop', interrumpiría la comprobación.
    $ErrorActionPreference = 'Continue'
    $line = & $cmd.Source @Arguments 2>$null | Select-Object -First 1
    if ($line) { return $line.ToString().Trim() } else { return $null }
}

function Install-Package([string]$Id, [string]$Version = '', [string[]]$Extra = @()) {
    Write-Host "    instalando $Id $Version"
    $wingetArgs = @('install', '--id', $Id, '--exact', '--source', 'winget', '--silent',
        '--accept-package-agreements', '--accept-source-agreements')
    if ($Version) { $wingetArgs += @('--version', $Version) }
    winget @wingetArgs @Extra
    # 0 = instalado; -1978335189 = ya instalado y sin actualización aplicable.
    if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne -1978335189) {
        throw "winget falló al instalar $Id (código $LASTEXITCODE)"
    }
    Update-SessionPath
}

function Get-WebView2Version {
    foreach ($key in "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\$WebView2Id",
                     "HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$WebView2Id") {
        $pv = (Get-ItemProperty $key -ErrorAction SilentlyContinue).pv
        if ($pv -and $pv -ne '0.0.0.0') { return $pv }
    }
    return $null
}

function Test-VsComponents {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vswhere)) { return $null }
    $vc = @(& $vswhere -products * -version '[17.0,18.0)' -requires $VcTools -property installationPath)
    $sdk = @(& $vswhere -products * -version '[17.0,18.0)' -requires $WindowsSdks -requiresAny -property installationPath)
    return $vc | Where-Object { $sdk -contains $_ } | Select-Object -First 1
}

$missing = New-Object System.Collections.Generic.List[string]
function Report([string]$Name, [string]$Found, [bool]$Ok, [string]$Expected) {
    $state = if ($Ok) { 'correcto' } elseif ($Found) { "se esperaba $Expected" } else { "falta ($Expected)" }
    Write-Host ("{0,-22} {1,-28} {2}" -f $Name, ($(if ($Found) { $Found } else { '-' })), $state)
    if (-not $Ok) { $missing.Add($Name) }
}

function Get-State {
    $python = Get-ToolLine 'python' @('--version')
    $node = Get-ToolLine 'node' @('--version')
    $rustup = Get-ToolLine 'rustup' @('--version')
    $rustc = $null
    if ($rustup) {
        # rustup elige la versión de rust-toolchain.toml dentro del repositorio.
        Push-Location $Root
        try { $rustc = Get-ToolLine 'rustc' @('--version') } finally { Pop-Location }
    }
    [pscustomobject]@{
        Python = $python; PythonOk = [bool]($python -match "^Python $([regex]::Escape($PythonSeries))\.")
        Node = $node; NodeOk = ($node -eq "v$NodeVersion")
        Rustup = $rustup
        Rustc = $rustc; RustcOk = [bool]($rustc -match "^rustc $([regex]::Escape($RustVersion)) ")
        Vs = Test-VsComponents
        WebView2 = Get-WebView2Version
    }
}

Update-SessionPath
$s = Get-State

if (-not $Comprobar) {
    Write-Host '==> Instalando lo que falte'
    if (-not $s.PythonOk) { Install-Package 'Python.Python.3.12' }
    if (-not $s.NodeOk) {
        # Una versión distinta de Node se sustituye por la fijada (winget actualiza o reinstala).
        Install-Package 'OpenJS.NodeJS.LTS' $NodeVersion
    }
    if (-not $s.Rustup) { Install-Package 'Rustlang.Rustup' }
    if (-not $s.RustcOk) {
        Write-Host "    instalando Rust $RustVersion (rust-toolchain.toml)"
        rustup toolchain install $RustVersion --profile minimal --component rustfmt,clippy
        if ($LASTEXITCODE -ne 0) { throw "rustup no pudo instalar Rust $RustVersion" }
    }
    if (-not $s.Vs) {
        $add = "--add $VcTools --add $($WindowsSdks[0])"
        Install-Package 'Microsoft.VisualStudio.2022.BuildTools' '' @('--override', "--wait --passive $add")
    }
    if (-not $s.WebView2) { Install-Package 'Microsoft.EdgeWebView2Runtime' }
    $s = Get-State
}

Write-Host ''
Write-Host '==> Estado de las herramientas'
Report 'Python' $s.Python $s.PythonOk "$PythonSeries.x"
Report 'Node.js' $s.Node $s.NodeOk "v$NodeVersion"
Report 'rustup' $s.Rustup ([bool]$s.Rustup) 'cualquier versión'
Report 'Rust (MSVC)' $s.Rustc $s.RustcOk $RustVersion
Report 'MSVC Build Tools 2022' $(if ($s.Vs) { 'instalados' } else { $null }) ([bool]$s.Vs) 'VC.Tools x64 y SDK de Windows 11'
Report 'WebView2 Runtime' $s.WebView2 ([bool]$s.WebView2) 'Evergreen'
$npm = Get-ToolLine 'npm' @('--version')
if ($npm) { Write-Host ("{0,-22} {1}" -f 'npm', $npm) }

if ($missing.Count -gt 0) {
    Write-Host ''
    if ($Comprobar) { Write-Host "Falta o no coincide: $($missing -join ', '). Ejecute el script sin -Comprobar." }
    else { Write-Host "No se pudo dejar listo: $($missing -join ', ')." }
    exit 1
}
if (-not $Comprobar) { Write-Host 'Abra una terminal nueva para que el PATH actualizado esté disponible.' }
