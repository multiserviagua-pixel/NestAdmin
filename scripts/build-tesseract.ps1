# Compila un Tesseract mínimo para el paquete (D69): solo PNG y zlib, sin libtiff, libjpeg,
# libwebp, giflib, OpenJPEG, libarchive, curl ni OpenMP, enlazado en estático con el runtime de
# MSVC (/MT). NestAdmin solo entrega PNG a Tesseract (páginas rasterizadas por PDFium e
# imágenes decodificadas por la aplicación, D67), así que no necesita más formatos. El
# resultado no lleva ninguna DLL GPL ni LGPL (JBIG-KIT, libiconv, runtime de GCC).
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-tesseract.ps1
#
# Requiere Git, CMake 3.21+ y Build Tools 2022 con MSVC x64 (scripts/setup-dev.ps1). Escribe en
# vendor/tesseract-min: tesseract.exe y las licencias de cada componente. No toca
# vendor/tesseract (la build de UB Mannheim) mientras D69 sea una propuesta.
param(
    [string]$Salida = (Join-Path (Split-Path -Parent $PSScriptRoot) 'vendor\tesseract-min')
)
$ErrorActionPreference = 'Stop'

# Fuentes fijadas por commit: una etiqueta movida o un espejo distinto hacen fallar el script.
$Sources = [ordered]@{
    zlib      = @{ Repo = 'https://github.com/madler/zlib'; Tag = 'v1.3.1'; Commit = '51b7f2abdade71cd9bb0e7a373ef2610ec6f9daf'; License = 'LICENSE' }
    libpng    = @{ Repo = 'https://github.com/pnggroup/libpng'; Tag = 'v1.6.44'; Commit = 'f5e92d76973a7a53f517579bc95d61483bf108c0'; License = 'LICENSE' }
    leptonica = @{ Repo = 'https://github.com/DanBloomberg/leptonica'; Tag = '1.85.0'; Commit = '63aef18d98432b8582a1565e241f7bd2ee9cc8d9'; License = 'leptonica-license.txt' }
    tesseract = @{ Repo = 'https://github.com/tesseract-ocr/tesseract'; Tag = '5.5.0'; Commit = '64eab6c457b2337dd690746a5fde5c222b40d5f8'; License = 'LICENSE' }
}

$Work = Join-Path ([IO.Path]::GetTempPath()) "nestadmin-tesseract-$([guid]::NewGuid().ToString('N').Substring(0, 8))"
$Prefix = Join-Path $Work 'prefix'
New-Item -ItemType Directory -Force $Prefix | Out-Null

function Invoke-Checked([string]$what, [scriptblock]$command) {
    & $command
    if ($LASTEXITCODE -ne 0) { throw "$what falló (código $LASTEXITCODE)" }
}

# Comunes a todos: Release, estático, runtime de MSVC enlazado (/MT) y Visual Studio x64.
$Common = @(
    '-G', 'Visual Studio 17 2022', '-A', 'x64',
    '-DCMAKE_BUILD_TYPE=Release',
    '-DBUILD_SHARED_LIBS=OFF',
    '-DCMAKE_POLICY_DEFAULT_CMP0091=NEW',
    '-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded',
    "-DCMAKE_INSTALL_PREFIX=$Prefix",
    "-DCMAKE_PREFIX_PATH=$Prefix"
)

function Build([string]$name, [string[]]$options) {
    $s = $Sources[$name]
    $src = Join-Path $Work $name
    Write-Host "==> $name $($s.Tag)"
    Invoke-Checked "git clone $name" { git -c advice.detachedHead=false clone --quiet --depth 1 --branch $s.Tag $s.Repo $src }
    $head = (git -C $src rev-parse HEAD).Trim()
    if ($head -ne $s.Commit) { throw "$name $($s.Tag) apunta a $head; se esperaba $($s.Commit)" }
    $build = Join-Path $Work "build-$name"
    Invoke-Checked "cmake $name" { cmake -S $src -B $build @Common @options | Out-Host }
    Invoke-Checked "compilación de $name" { cmake --build $build --config Release --parallel | Out-Host }
    Invoke-Checked "instalación de $name" { cmake --install $build --config Release | Out-Host }
}

try {
    Build 'zlib' @('-DZLIB_BUILD_EXAMPLES=OFF')
    # zlib instala también la biblioteca dinámica: se retira para que nadie enlace con ella.
    Remove-Item (Join-Path $Prefix 'bin\zlib*.dll'), (Join-Path $Prefix 'lib\zlib.lib') -ErrorAction SilentlyContinue
    Copy-Item (Join-Path $Prefix 'lib\zlibstatic.lib') (Join-Path $Prefix 'lib\zlib.lib')

    Build 'libpng' @('-DPNG_SHARED=OFF', '-DPNG_STATIC=ON', '-DPNG_TESTS=OFF', '-DPNG_TOOLS=OFF', "-DZLIB_ROOT=$Prefix")
    Copy-Item (Join-Path $Prefix 'lib\libpng16_static.lib') (Join-Path $Prefix 'lib\libpng16.lib') -Force

    Build 'leptonica' @('-DSW_BUILD=OFF', '-DBUILD_PROG=OFF', '-DENABLE_ZLIB=ON', '-DENABLE_PNG=ON',
        '-DENABLE_GIF=OFF', '-DENABLE_JPEG=OFF', '-DENABLE_TIFF=OFF', '-DENABLE_WEBP=OFF', '-DENABLE_OPENJPEG=OFF')

    Build 'tesseract' @('-DSW_BUILD=OFF', '-DOPENMP_BUILD=OFF', '-DBUILD_TRAINING_TOOLS=OFF',
        '-DDISABLE_TIFF=ON', '-DDISABLE_ARCHIVE=ON', '-DDISABLE_CURL=ON', '-DGRAPHICS_DISABLED=ON',
        '-DWIN32_MT_BUILD=ON', '-DINSTALL_CONFIGS=OFF')

    $exe = Join-Path $Prefix 'bin\tesseract.exe'
    if (-not (Test-Path $exe)) { throw 'No se generó tesseract.exe' }

    # Solo DLL del sistema: ninguna de terceros ni del runtime de Visual C++ o de GCC.
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    $vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    $dumpbin = Get-ChildItem (Join-Path $vs 'VC\Tools\MSVC') -Recurse -Filter dumpbin.exe |
        Where-Object { $_.FullName -like '*Hostx64\x64*' } | Select-Object -First 1
    $deps = @(& $dumpbin.FullName /dependents $exe | Where-Object { $_ -match '^\s+\S+\.dll\s*$' } | ForEach-Object { $_.Trim() })
    Write-Host "==> Dependencias: $($deps -join ', ')"
    $system = '^(KERNEL32|ADVAPI32|USER32|GDI32|SHELL32|OLE32|OLEAUT32|WS2_32|SHLWAPI|bcrypt|api-ms-win-.*)\.dll$'
    $foreign = @($deps | Where-Object { $_ -notmatch $system })
    if ($foreign) { throw "tesseract.exe depende de DLL que no son del sistema: $($foreign -join ', ')" }

    New-Item -ItemType Directory -Force $Salida | Out-Null
    Copy-Item $exe $Salida -Force
    $licenses = Join-Path $Salida 'licenses'
    New-Item -ItemType Directory -Force $licenses | Out-Null
    foreach ($name in $Sources.Keys) {
        Copy-Item (Join-Path (Join-Path $Work $name) $Sources[$name].License) (Join-Path $licenses "$name.txt") -Force
    }
    $versions = $Sources.Keys | ForEach-Object { "$_ $($Sources[$_].Tag) $($Sources[$_].Commit)" }
    Set-Content -Path (Join-Path $Salida 'VERSIONS.txt') -Value $versions -Encoding UTF8

    Write-Host '==> Comprobación'
    $out = Join-Path $Salida 'tesseract.exe'
    & $out --version
    $tessdata = Join-Path (Split-Path -Parent $PSScriptRoot) 'vendor\tessdata'
    if (Test-Path $tessdata) {
        Invoke-Checked 'tesseract --list-langs' { & $out --tessdata-dir $tessdata --list-langs }
    }
    Write-Host "==> Listo en $Salida. Pruebas: `$env:NESTADMIN_TESSERACT='$out'; cargo test -p nestadmin-extract -- --include-ignored"
} finally {
    Remove-Item $Work -Recurse -Force -ErrorAction SilentlyContinue
}
