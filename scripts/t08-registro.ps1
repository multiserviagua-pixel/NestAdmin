# Registro de evidencia para T08 (pruebas-instalador.md). Se ejecuta DENTRO de la máquina
# virtual de prueba, nunca en el equipo de desarrollo. Anota el equipo antes y después de
# instalar o desinstalar NestAdmin en un archivo de texto en el escritorio.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File t08-registro.ps1                  # instalación con interfaz (T08-01)
#   powershell -NoProfile -ExecutionPolicy Bypass -File t08-registro.ps1 -Silencioso      # instalación /S (T08-08)
#   powershell -NoProfile -ExecutionPolicy Bypass -File t08-registro.ps1 -Accion Estado   # solo el estado actual
#   powershell -NoProfile -ExecutionPolicy Bypass -File t08-registro.ps1 -Accion Desinstalar
#
# No anota el nombre del equipo ni del usuario: las rutas personales se escriben como %USERPROFILE%.
param(
    [ValidateSet('Instalar', 'Desinstalar', 'Estado')]
    [string]$Accion = 'Instalar',
    [string]$Instalador = 'C:\Instalador\NestAdmin_0.1.0_x64-setup.exe',
    [switch]$Silencioso
)
$ErrorActionPreference = 'Continue'

$InstallDir = Join-Path $env:LOCALAPPDATA 'NestAdmin'
$OllamaExe = Join-Path $env:LOCALAPPDATA 'Programs\Ollama\ollama.exe'
$Stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$Log = Join-Path ([Environment]::GetFolderPath('Desktop')) "t08-$($Accion.ToLower())-$Stamp.txt"

function Hide-Personal([string]$text) {
    if (-not $text) { return $text }
    $text.Replace($env:USERPROFILE, '%USERPROFILE%')
}

function Write-Log([string]$line) {
    $line = Hide-Personal $line
    Write-Host $line
    Add-Content -Path $Log -Value $line -Encoding UTF8
}

function Get-Json([string]$path) {
    try { Invoke-RestMethod "http://127.0.0.1:11434$path" -TimeoutSec 5 } catch { $null }
}

function Get-WebView2Version {
    $guid = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
    foreach ($key in @(
            "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\$guid",
            "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$guid",
            "HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$guid")) {
        $pv = (Get-ItemProperty $key -ErrorAction SilentlyContinue).pv
        if ($pv -and $pv -ne '0.0.0.0') { return "$pv ($($key.Split(':')[0]))" }
    }
    'no instalado'
}

function Write-State([string]$title) {
    Write-Log ''
    Write-Log "== $title ($(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')) =="
    $os = Get-CimInstance Win32_OperatingSystem
    $cv = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
    Write-Log "Windows: $($os.Caption) $($cv.DisplayVersion), compilación $($os.BuildNumber).$($cv.UBR)"
    $cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1).Name.Trim()
    Write-Log ("Hardware: {0}; {1:N1} GB de RAM" -f $cpu, ($os.TotalVisibleMemorySize / 1MB))
    $disk = Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='C:'"
    Write-Log ("Disco C: libre {0:N1} GB" -f ($disk.FreeSpace / 1GB))
    try {
        $null = Invoke-WebRequest 'https://github.com' -Method Head -UseBasicParsing -TimeoutSec 10
        Write-Log 'Internet: responde github.com'
    } catch { Write-Log 'Internet: sin acceso a github.com' }
    Write-Log "WebView2: $(Get-WebView2Version)"

    if (Test-Path $OllamaExe) {
        $v = (& $OllamaExe --version 2>&1 | Out-String).Trim() -replace '\s+', ' '
        Write-Log "Ollama instalado: $OllamaExe ($v)"
    } else { Write-Log 'Ollama instalado: no' }
    $version = Get-Json '/api/version'
    if ($version) {
        Write-Log "Ollama en marcha: versión $($version.version)"
        $tags = Get-Json '/api/tags'
        $models = @($tags.models | ForEach-Object { "$($_.name) ($($_.digest.Substring(0, 12)))" })
        Write-Log "Modelos: $(if ($models.Count) { $models -join ', ' } else { 'ninguno' })"
    } else { Write-Log 'Ollama en marcha: no responde en 127.0.0.1:11434' }
    $procs = @(Get-Process | Where-Object { $_.ProcessName -like 'ollama*' -or $_.ProcessName -eq 'nestadmin' } |
        ForEach-Object { "$($_.ProcessName) ($($_.Id))" })
    Write-Log "Procesos: $(if ($procs.Count) { $procs -join ', ' } else { 'ninguno' })"

    if (Test-Path $InstallDir) {
        $items = Get-ChildItem $InstallDir -Name
        Write-Log "Carpeta NestAdmin: $($items -join ', ')"
        $tess = Join-Path $InstallDir 'tesseract'
        if (Test-Path $tess) { Write-Log "  tesseract\: $((Get-ChildItem $tess).Count) archivos" }
        $exe = Join-Path $tess 'tesseract.exe'
        if (Test-Path $exe) {
            $langs = (& $exe --tessdata-dir (Join-Path $InstallDir 'tessdata') --list-langs 2>&1 | Select-Object -Skip 1) -join ', '
            Write-Log "  Tesseract instalado lista: $langs"
        }
    } else { Write-Log 'Carpeta NestAdmin: no existe' }
    $uninstall = Get-ChildItem 'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall' -ErrorAction SilentlyContinue |
        ForEach-Object { Get-ItemProperty $_.PSPath } | Where-Object { $_.DisplayName -like 'NestAdmin*' }
    Write-Log "Aplicaciones instaladas: $(if ($uninstall) { "$($uninstall.DisplayName) $($uninstall.DisplayVersion)" } else { 'sin entrada de NestAdmin' })"
    $menu = Get-ChildItem (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs') -Recurse -Filter 'NestAdmin*.lnk' -ErrorAction SilentlyContinue
    Write-Log "Menú Inicio: $(if ($menu) { 'acceso directo presente' } else { 'sin acceso directo' })"
}

function Invoke-Timed([string]$file, [string[]]$arguments, [string]$label) {
    Write-Log ''
    Write-Log "== $label =="
    Write-Log "Orden: `"$file`" $($arguments -join ' ')"
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $p = if ($arguments) { Start-Process $file -ArgumentList $arguments -Wait -PassThru } else { Start-Process $file -Wait -PassThru }
    $watch.Stop()
    Write-Log ("Código de salida: {0}; duración {1:N1} s ({2:mm\:ss})" -f $p.ExitCode, $watch.Elapsed.TotalSeconds, $watch.Elapsed)
}

Write-Log "Registro T08 — acción: $Accion$(if ($Silencioso) { ' (silenciosa)' })"
Write-State 'Antes'

switch ($Accion) {
    'Instalar' {
        if (-not (Test-Path $Instalador)) { Write-Log "No existe el instalador $Instalador"; exit 2 }
        $item = Get-Item $Instalador
        Write-Log ''
        Write-Log "Instalador: $($item.Name), $($item.Length) bytes, SHA-256 $((Get-FileHash $Instalador -Algorithm SHA256).Hash)"
        Write-Log "Firma del instalador: $((Get-AuthenticodeSignature $Instalador).Status)"
        $arguments = if ($Silencioso) { @('/S') } else { @() }
        Invoke-Timed $Instalador $arguments 'Instalación'
    }
    'Desinstalar' {
        $uninstaller = Join-Path $InstallDir 'uninstall.exe'
        if (-not (Test-Path $uninstaller)) { Write-Log 'NestAdmin no está instalado'; exit 2 }
        $arguments = if ($Silencioso) { @('/S') } else { @() }
        Invoke-Timed $uninstaller $arguments 'Desinstalación'
        Start-Sleep -Seconds 3   # el desinstalador de NSIS termina de borrar en otro proceso
    }
}

if ($Accion -ne 'Estado') { Write-State 'Después' }
Write-Log ''
Write-Log "Registro guardado en $Log"
Write-Log 'Añada a mano lo que vio: ventanas del asistente, avisos (SmartScreen, WebView2), errores y reintentos.'
