# Construcción desde un entorno limpio (R11, T11): clona el repositorio en una carpeta nueva
# y ejecuta allí, en orden, los pasos documentados en README.md: preparación de PDF/OCR,
# comprobaciones, interfaz, formato, análisis, pruebas e instalador. Usa un directorio de
# compilación propio. Al final comprueba que nada generado queda fuera de lo ignorado por
# Git y que las pruebas no han escrito en la carpeta de datos del usuario.
#
# Reutiliza las herramientas ya instaladas (scripts/setup-dev.ps1) y las cachés de descargas
# de Cargo y npm; no modifica el repositorio de origen.
param(
    # Repositorio a clonar: ruta local o URL. Por defecto, este repositorio.
    [string]$Source = (Split-Path -Parent $PSScriptRoot),
    # Rama o etiqueta. Por defecto, la rama actual del origen local.
    [string]$Ref = '',
    # Carpeta nueva donde clonar. Por defecto, una carpeta temporal única.
    [string]$Destination = (Join-Path ([IO.Path]::GetTempPath()) "nestadmin-limpio-$([guid]::NewGuid().ToString('N').Substring(0, 8))"),
    [switch]$SkipInstaller
)
$ErrorActionPreference = 'Stop'

$env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' +
    [Environment]::GetEnvironmentVariable('Path', 'User') + ';' +
    (Join-Path $env:USERPROFILE '.cargo\bin')

if (Test-Path $Destination) { throw "La carpeta de destino ya existe: $Destination" }
if (-not $Ref -and (Test-Path (Join-Path $Source '.git'))) {
    $Ref = (git -C $Source rev-parse --abbrev-ref HEAD).Trim()
}

$userData = Join-Path $env:LOCALAPPDATA 'NestAdmin'
$userDataBefore = Test-Path $userData
$steps = New-Object System.Collections.Generic.List[object]

function Invoke-Step([string]$Name, [scriptblock]$Command) {
    Write-Host ""
    Write-Host "==> $Name"
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $global:LASTEXITCODE = 0
    & $Command
    $code = $LASTEXITCODE
    $watch.Stop()
    $steps.Add([pscustomobject]@{ Paso = $Name; Segundos = [math]::Round($watch.Elapsed.TotalSeconds) })
    if ($code -ne 0) { throw "Falló «$Name» (código $code)" }
}

Write-Host "==> Clonando $Source ($Ref) en $Destination"
$cloneArgs = @('clone', '--quiet', '--no-local')
if ($Ref) { $cloneArgs += @('--branch', $Ref) }
git @cloneArgs $Source $Destination
if ($LASTEXITCODE -ne 0) { throw 'No se pudo clonar el repositorio' }
$commit = (git -C $Destination rev-parse --short HEAD).Trim()

Push-Location $Destination
try {
    $env:CARGO_TARGET_DIR = Join-Path $Destination 'target'

    Invoke-Step 'Versiones de herramientas' {
        git --version; python --version; node --version; npm --version
        rustup --version; cargo --version; rustc --version
    }
    Invoke-Step 'PDFium, Tesseract y datos de idioma (scripts/setup-ocr.ps1)' {
        powershell -NoProfile -ExecutionPolicy Bypass -File scripts/setup-ocr.ps1
    }
    Invoke-Step 'Avisos de terceros (scripts/third_party.py, D60)' {
        python scripts/third_party.py --check
        if ($LASTEXITCODE -eq 0) { python scripts/third_party.py --textos vendor/THIRD_PARTY_NOTICES.txt }
    }
    Invoke-Step 'Especificación (scripts/check.py)' { python scripts/check.py }
    Invoke-Step 'Pruebas del verificador' { python -m unittest discover -s scripts/tests }
    Invoke-Step 'Pruebas del evaluador' { python -m unittest discover -s evaluation/tests }
    Invoke-Step 'Dependencias de la interfaz (npm ci)' { npm ci --prefix ui --no-audit --no-fund }
    Invoke-Step 'Interfaz (npm run build)' { npm run build --prefix ui }
    Invoke-Step 'Formato (cargo fmt)' { cargo fmt --all --check }
    Invoke-Step 'Análisis (cargo clippy)' { cargo clippy --workspace --all-targets --locked -- -D warnings }
    Invoke-Step 'Pruebas (cargo test)' { cargo test --workspace --locked }
    if (-not $SkipInstaller) {
        Invoke-Step 'Instalador (tauri build)' {
            Push-Location crates/desktop
            try { & ..\..\ui\node_modules\.bin\tauri.cmd build --config tauri.installer.json } finally { Pop-Location }
        }
    }

    Invoke-Step 'Separación de resultados y archivos generados' {
        # Todo lo generado debe estar ignorado: el árbol de trabajo sigue limpio.
        $dirty = git status --porcelain
        if ($dirty) { Write-Host $dirty; throw 'La construcción dejó archivos sin ignorar en el repositorio' }
        # Los resultados de evaluación y los binarios descargados nunca se versionan.
        foreach ($path in 'evaluation/results/informe.json', 'evaluation/predictions/ollama.jsonl', 'vendor/pdfium/bin/pdfium.dll') {
            git check-ignore -q $path
            if ($LASTEXITCODE -ne 0) { throw "$path no está ignorado por Git" }
        }
        # Las pruebas usan carpetas temporales, nunca los datos del usuario.
        if (-not $userDataBefore -and (Test-Path $userData)) { throw "Las pruebas crearon $userData" }
        $global:LASTEXITCODE = 0
    }
} finally {
    Pop-Location
    Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Host "==> Construcción limpia correcta: commit $commit en $Destination"
$steps | Format-Table -AutoSize | Out-String | Write-Host
if (-not $SkipInstaller) {
    $installer = Get-ChildItem (Join-Path $Destination 'target\release\bundle\nsis\*.exe') | Select-Object -First 1
    Write-Host "Instalador: $($installer.FullName)"
    Write-Host "Tamaño: $($installer.Length) bytes"
    Write-Host "SHA-256: $((Get-FileHash $installer.FullName).Hash)"
}
