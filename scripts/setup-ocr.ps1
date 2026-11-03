# Prepara PDF y OCR locales para desarrollo (D09, D10): descarga PDFium fijado y comprobado
# por SHA-256 en vendor/pdfium e instala Tesseract 5 mediante winget. Se puede repetir:
# reutiliza lo ya presente. El instalador final de la aplicación (R08) tendrá su propio flujo.
$ErrorActionPreference = 'Stop'

$PdfiumRelease = 'chromium/8125'
$PdfiumSha256 = '9a964261760cff52570aabf038ab346d3a5034dd8b30d0d5fe8861235d356b8b'
$Root = Split-Path -Parent $PSScriptRoot
$Vendor = Join-Path $Root 'vendor\pdfium'
$Dll = Join-Path $Vendor 'bin\pdfium.dll'

if (Test-Path $Dll) {
    Write-Host "==> PDFium ya presente en $Vendor"
} else {
    Write-Host "==> Descargando PDFium $PdfiumRelease"
    $archive = Join-Path ([IO.Path]::GetTempPath()) "pdfium-$([guid]::NewGuid()).tgz"
    try {
        $url = "https://github.com/bblanchon/pdfium-binaries/releases/download/$PdfiumRelease/pdfium-win-x64.tgz"
        Invoke-WebRequest -Uri $url -OutFile $archive -UseBasicParsing
        $hash = (Get-FileHash -Algorithm SHA256 $archive).Hash.ToLowerInvariant()
        if ($hash -ne $PdfiumSha256) { throw "SHA-256 inesperado para PDFium: $hash" }
        New-Item -ItemType Directory -Force $Vendor | Out-Null
        # tar de Windows: otro tar en el PATH (Git Bash) no entiende rutas con unidad.
        & "$env:SystemRoot\System32\tar.exe" -xzf $archive -C $Vendor
        if ($LASTEXITCODE -ne 0) { throw 'No se pudo extraer PDFium' }
    } finally {
        Remove-Item $archive -ErrorAction SilentlyContinue
    }
}

$env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' +
    [Environment]::GetEnvironmentVariable('Path', 'User')
$tesseract = Get-Command tesseract -ErrorAction SilentlyContinue
if (-not $tesseract -and (Test-Path 'C:\Program Files\Tesseract-OCR\tesseract.exe')) {
    $tesseract = Get-Item 'C:\Program Files\Tesseract-OCR\tesseract.exe'
}
if ($tesseract) {
    Write-Host '==> Tesseract ya instalado'
} else {
    Write-Host '==> Instalando Tesseract 5 (UB Mannheim)'
    winget install --id UB-Mannheim.TesseractOCR --exact --source winget --silent --accept-package-agreements --accept-source-agreements
    # 0 = instalado; -1978335189 = ya instalado y sin actualización aplicable.
    if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne -1978335189) {
        throw "winget falló al instalar Tesseract (código $LASTEXITCODE)"
    }
}

# El instalador silencioso solo trae inglés: los datos de idioma de NestAdmin (spa, eng)
# se guardan aparte, fijados y comprobados, y se pasan a Tesseract con --tessdata-dir.
$Tessdata = Join-Path $Root 'vendor\tessdata'
$Languages = @{
    'spa' = '0b0fcbb4665189e01ab8019e591f014dd7260460de072543edd4b2cb4ede7c96'
    'eng' = 'daa0c97d651c19fba3b25e81317cd697e9908c8208090c94c3905381c23fc047'
}
New-Item -ItemType Directory -Force $Tessdata | Out-Null
foreach ($lang in $Languages.Keys) {
    $file = Join-Path $Tessdata "$lang.traineddata"
    if (Test-Path $file) { Write-Host "==> Datos de idioma $lang ya presentes"; continue }
    Write-Host "==> Descargando datos de idioma $lang (tessdata 4.1.0)"
    $partial = "$file.part"
    Invoke-WebRequest -Uri "https://github.com/tesseract-ocr/tessdata/raw/4.1.0/$lang.traineddata" -OutFile $partial -UseBasicParsing
    $hash = (Get-FileHash -Algorithm SHA256 $partial).Hash.ToLowerInvariant()
    if ($hash -ne $Languages[$lang]) { Remove-Item $partial; throw "SHA-256 inesperado para $lang`: $hash" }
    Move-Item $partial $file
}

Write-Host '==> Comprobación'
Get-Content (Join-Path $Vendor 'VERSION')
$exe = 'C:\Program Files\Tesseract-OCR\tesseract.exe'
if (Test-Path $exe) { & $exe --version | Select-Object -First 1; & $exe --tessdata-dir $Tessdata --list-langs }
