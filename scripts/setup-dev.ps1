# Instala las herramientas de desarrollo de NestAdmin en Windows mediante winget.
# Reutiliza lo que ya esté instalado; se puede volver a ejecutar sin efectos duplicados.
# No instala Ollama, modelos ni OCR: se añaden en la fase que los integra.
$ErrorActionPreference = 'Stop'

function Update-SessionPath {
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' +
        [Environment]::GetEnvironmentVariable('Path', 'User') + ';' +
        (Join-Path $env:USERPROFILE '.cargo\bin')
}

function Test-Tool([string]$Name) {
    $cmd = Get-Command $Name -ErrorAction SilentlyContinue
    # El alias de la Microsoft Store para python no es un intérprete real.
    return $cmd -and $cmd.Source -notmatch '\\WindowsApps\\'
}

function Install-Package([string]$Id, [string[]]$Extra = @()) {
    Write-Host "==> Instalando $Id"
    winget install --id $Id --exact --source winget --silent --accept-package-agreements --accept-source-agreements @Extra
    # 0 = instalado; -1978335189 = ya instalado y sin actualización aplicable.
    if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne -1978335189) {
        throw "winget falló al instalar $Id (código $LASTEXITCODE)"
    }
}

Update-SessionPath

if (-not (Test-Tool 'python')) { Install-Package 'Python.Python.3.12' } else { Write-Host '==> Python ya instalado' }
if (-not (Test-Tool 'node')) { Install-Package 'OpenJS.NodeJS.LTS' } else { Write-Host '==> Node.js ya instalado' }
if (-not (Test-Tool 'rustup')) { Install-Package 'Rustlang.Rustup' } else { Write-Host '==> rustup ya instalado' }

# Compilador MSVC y Windows SDK que necesita Rust/Tauri.
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$hasVc = (Test-Path $vswhere) -and (& $vswhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
if (-not $hasVc) {
    Install-Package 'Microsoft.VisualStudio.2022.BuildTools' @('--override', '--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended')
} else { Write-Host '==> MSVC Build Tools ya instalados' }

Update-SessionPath
rustup default stable-x86_64-pc-windows-msvc | Out-Null

Write-Host ''
Write-Host '==> Versiones detectadas'
python --version
node --version
npm --version
rustc --version
cargo --version
Write-Host 'Abra una terminal nueva para que el PATH actualizado esté disponible.'
