# Registro de evidencia para T08 (pruebas-instalador.md). Se ejecuta DENTRO de la máquina
# virtual de prueba, nunca en el equipo de desarrollo. Anota el equipo antes y después de
# instalar o desinstalar NestAdmin en un archivo de texto en el escritorio.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File t08-registro.ps1                  # instalación con interfaz (T08-01)
#   powershell -NoProfile -ExecutionPolicy Bypass -File t08-registro.ps1 -Silencioso      # instalación /S (T08-08)
#   powershell -NoProfile -ExecutionPolicy Bypass -File t08-registro.ps1 -Accion Estado   # solo el estado actual
#   powershell -NoProfile -ExecutionPolicy Bypass -File t08-registro.ps1 -Accion Desinstalar
#
# Espera solo al proceso del instalador, no a sus descendientes: `Start-Process -Wait` de Windows
# PowerShell espera a todo el árbol, y el instalador deja en marcha `ollama serve` (lo arranca
# `nestadmin.exe --preparar`) y, si se marca al terminar, NestAdmin en el área de notificación.
# Con -Wait el script no volvía nunca y no anotaba el estado «Después».
#
# No anota el nombre del equipo ni del usuario: las rutas personales se escriben como %USERPROFILE%.
param(
    [ValidateSet('Instalar', 'Desinstalar', 'Estado')]
    [string]$Accion = 'Instalar',
    [string]$Instalador = 'C:\Instalador\NestAdmin_0.1.0_x64-setup.exe',
    [switch]$Silencioso,
    # Plazo para el instalador o desinstalador; la descarga del modelo (5,2 GB) va incluida.
    [int]$PlazoMinutos = 120,
    # Modelo que se comprueba con una respuesta real al final.
    [string]$Modelo = 'qwen3:8b'
)
$ErrorActionPreference = 'Continue'

# Programa y datos van en carpetas distintas (D52). La carpeta del programa se lee del registro,
# por si se instaló en otra; los datos siempre están en %LOCALAPPDATA%\NestAdmin.
$DataDir = Join-Path $env:LOCALAPPDATA 'NestAdmin'
$DefaultInstallDir = Join-Path $env:LOCALAPPDATA 'Programs\NestAdmin'
function Get-InstallDir {
    $key = 'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\NestAdmin'
    $location = (Get-ItemProperty $key -ErrorAction SilentlyContinue).InstallLocation
    if ($location) { return $location.Trim('"') }
    $DefaultInstallDir
}
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

    $InstallDir = Get-InstallDir
    Write-Log "Carpeta del programa: $InstallDir"
    if (Test-Path $InstallDir) {
        $items = Get-ChildItem $InstallDir -Name
        Write-Log "  Contenido: $($items -join ', ')"
        $oauth = if (Test-Path (Join-Path $InstallDir 'correo-oauth.json')) { 'sí (no se anota su contenido)' } else { 'no' }
        Write-Log "  correo-oauth.json junto al ejecutable: $oauth"
        $tess = Join-Path $InstallDir 'tesseract'
        if (Test-Path $tess) { Write-Log "  tesseract\: $((Get-ChildItem $tess).Count) archivos" }
        $exe = Join-Path $tess 'tesseract.exe'
        if (Test-Path $exe) {
            $langs = (& $exe --tessdata-dir (Join-Path $InstallDir 'tessdata') --list-langs 2>&1 | Select-Object -Skip 1) -join ', '
            Write-Log "  Tesseract instalado lista: $langs"
        }
    } else { Write-Log '  No existe' }
    if (Test-Path $DataDir) {
        Write-Log "Carpeta de datos ${DataDir}: $((Get-ChildItem $DataDir -Name) -join ', ')"
        if ($DataDir -ne $InstallDir) {
            foreach ($leftover in 'nestadmin.exe', 'uninstall.exe', 'tesseract', 'pdfium', 'tessdata') {
                if (Test-Path (Join-Path $DataDir $leftover)) {
                    Write-Log "  AVISO: queda $leftover del programa en la carpeta de datos (copia duplicada)"
                }
            }
        }
    } else { Write-Log "Carpeta de datos ${DataDir}: no existe" }
    $uninstall = Get-ChildItem 'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall' -ErrorAction SilentlyContinue |
        ForEach-Object { Get-ItemProperty $_.PSPath } | Where-Object { $_.DisplayName -like 'NestAdmin*' }
    Write-Log "Aplicaciones instaladas: $(if ($uninstall) { "$($uninstall.DisplayName) $($uninstall.DisplayVersion)" } else { 'sin entrada de NestAdmin' })"
    $menu = Get-ChildItem (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs') -Recurse -Filter 'NestAdmin*.lnk' -ErrorAction SilentlyContinue
    Write-Log "Menú Inicio: $(if ($menu) { 'acceso directo presente' } else { 'sin acceso directo' })"
    $desktopLink = Get-Item (Join-Path ([Environment]::GetFolderPath('Desktop')) 'NestAdmin.lnk') -ErrorAction SilentlyContinue
    $shell = New-Object -ComObject WScript.Shell
    foreach ($link in @($menu) + @($desktopLink) | Where-Object { $_ }) {
        $target = $shell.CreateShortcut($link.FullName).TargetPath
        $state = if (Test-Path $target) { 'existe' } else { 'ROTO' }
        Write-Log "  Acceso $($link.Name) -> $target ($state)"
    }
}

function Invoke-Timed([string]$file, [string[]]$arguments, [string]$label) {
    Write-Log ''
    Write-Log "== $label =="
    Write-Log "Orden: `"$file`" $($arguments -join ' ')"
    $before = @(Get-Process | ForEach-Object { $_.Id })
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $p = if ($arguments) { Start-Process $file -ArgumentList $arguments -PassThru } else { Start-Process $file -PassThru }
    # Sin leer Handle antes de esperar, ExitCode queda vacío en Windows PowerShell.
    $null = $p.Handle
    $finished = $p.WaitForExit($PlazoMinutos * 60 * 1000)
    $watch.Stop()
    if ($finished) {
        Write-Log ("Código de salida: {0}; duración {1:N1} s ({2:mm\:ss})" -f $p.ExitCode, $watch.Elapsed.TotalSeconds, $watch.Elapsed)
    } else {
        Write-Log "SIN TERMINAR tras $PlazoMinutos min: el proceso sigue abierto (¿un diálogo esperando respuesta?). Se anota el estado igualmente."
    }
    $spawned = @(Get-Process | Where-Object { $before -notcontains $_.Id -and $_.Id -ne $p.Id } |
        Where-Object { $_.ProcessName -like 'ollama*' -or $_.ProcessName -eq 'nestadmin' -or $_.ProcessName -like '*setup*' } |
        ForEach-Object { "$($_.ProcessName) ($($_.Id))" })
    Write-Log "Procesos que siguen en marcha tras $($label.ToLower()): $(if ($spawned.Count) { $spawned -join ', ' } else { 'ninguno' })"
}

function Test-Model {
    Write-Log ''
    Write-Log "== Comprobación de la IA local ($Modelo) =="
    if (-not (Get-Json '/api/version')) { Write-Log 'Resultado: Ollama no responde en 127.0.0.1:11434'; return }
    $body = @{
        model    = $Modelo
        stream   = $false
        think    = $false
        options  = @{ temperature = 0 }
        messages = @(@{ role = 'user'; content = 'Responde solo con la palabra: listo' })
    } | ConvertTo-Json -Depth 5
    $watch = [Diagnostics.Stopwatch]::StartNew()
    try {
        $r = Invoke-RestMethod 'http://127.0.0.1:11434/api/chat' -Method Post -Body ([Text.Encoding]::UTF8.GetBytes($body)) `
            -ContentType 'application/json; charset=utf-8' -TimeoutSec 300
        $watch.Stop()
        $answer = ("$($r.message.content)".Trim() -replace '\s+', ' ')
        if ($answer.Length -gt 60) { $answer = $answer.Substring(0, 60) + '…' }
        $state = if ($answer) { 'OK' } else { 'FALLO (respuesta vacía)' }
        Write-Log ("Resultado: {0}; {1:N1} s; respuesta «{2}»" -f $state, $watch.Elapsed.TotalSeconds, $answer)
    } catch {
        $watch.Stop()
        Write-Log ("Resultado: FALLO tras {0:N1} s: {1}" -f $watch.Elapsed.TotalSeconds, $_.Exception.Message)
    }
}

Write-Host "El registro se escribe en $(Hide-Personal $Log)"
Write-Log "Registro T08 — acción: $Accion$(if ($Silencioso) { ' (silenciosa)' })"
Write-State 'Antes'

$exitCode = 0
try {
switch ($Accion) {
    'Instalar' {
        if (-not (Test-Path $Instalador)) { Write-Log "No existe el instalador $Instalador (use -Instalador RUTA)"; $exitCode = 2; break }
        $item = Get-Item $Instalador
        Write-Log ''
        Write-Log "Instalador: $($item.Name), $($item.Length) bytes, SHA-256 $((Get-FileHash $Instalador -Algorithm SHA256).Hash)"
        Write-Log "Firma del instalador: $((Get-AuthenticodeSignature $Instalador).Status)"
        $arguments = if ($Silencioso) { @('/S') } else { @() }
        Invoke-Timed $Instalador $arguments 'Instalación'
    }
    'Desinstalar' {
        $uninstaller = Join-Path (Get-InstallDir) 'uninstall.exe'
        if (-not (Test-Path $uninstaller)) { Write-Log 'NestAdmin no está instalado'; $exitCode = 2; break }
        $arguments = if ($Silencioso) { @('/S') } else { @() }
        Invoke-Timed $uninstaller $arguments 'Desinstalación'
        Start-Sleep -Seconds 3   # el desinstalador de NSIS termina de borrar en otro proceso
    }
}
} catch {
    Write-Log "ERROR del script: $($_.Exception.Message)"
    $exitCode = 1
} finally {
    # También tras Ctrl+C o un error: lo anotado hasta aquí no se pierde.
    if ($Accion -ne 'Estado' -and $exitCode -ne 2) { Write-State 'Después' }
    if ($Accion -ne 'Desinstalar') { Test-Model }
}
Write-Log ''
Write-Log "Registro guardado en $Log"
Write-Log 'Añada a mano lo que vio: ventanas del asistente, avisos (SmartScreen, WebView2), errores y reintentos.'
exit $exitCode
