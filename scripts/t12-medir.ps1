# T12: ejecuta el corpus sintético con un proveedor y mide la memoria en el equipo objetivo.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/t12-medir.ps1 -Proveedor ollama
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/t12-medir.ps1 -Proveedor groq -Modelo qwen/qwen3.8-27b
#
# Mientras corre nestadmin-predict, cada medio segundo suma el conjunto de trabajo de los
# procesos de Ollama (ollama*, incluido el que carga el modelo) y el del propio cliente, y guarda
# el máximo. Con Groq la memoria del servidor remoto no es observable: solo se mide el cliente.
# Escribe las predicciones en evaluation/predictions/, el informe en evaluation/results/ y un
# resumen en evaluation/results/t12-<proveedor>-<fecha>.txt, todo fuera de Git. No anota datos
# personales: equipo y usuario no constan.
param(
    [Parameter(Mandatory = $true)][ValidateSet('ollama', 'groq')][string]$Proveedor,
    [string]$Modelo = '',
    [int]$PlazoSegundos = 300
)
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

if ($Proveedor -eq 'groq' -and -not $Modelo) { throw 'Con groq indique -Modelo (por ejemplo qwen/qwen3.8-27b)' }
$name = if ($Modelo) { "$Proveedor-$($Modelo -replace '[^A-Za-z0-9.-]', '_')" } else { $Proveedor }
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$out = "evaluation/predictions/t12-$name-$stamp.jsonl"
New-Item -ItemType Directory -Force evaluation/predictions, evaluation/results | Out-Null

Write-Host '==> Compilando nestadmin-predict (release)'
cargo build --release --locked -p nestadmin-ai --bin nestadmin-predict
if ($LASTEXITCODE -ne 0) { throw 'No se pudo compilar nestadmin-predict' }

$arguments = @('--provider', $Proveedor, '--out', $out, '--timeout-secs', "$PlazoSegundos")
if ($Modelo) { $arguments += @('--model', $Modelo) }

# Memoria antes de empezar: el modelo puede estar ya cargado (se anota, no se resta).
function Get-Sample($clientId) {
    $ollama = (Get-Process -Name 'ollama*' -ErrorAction SilentlyContinue | Measure-Object WorkingSet64 -Sum).Sum
    $client = if ($clientId) { (Get-Process -Id $clientId -ErrorAction SilentlyContinue).WorkingSet64 } else { 0 }
    [pscustomobject]@{ Ollama = [int64]$ollama; Client = [int64]$client }
}
$before = Get-Sample $null
$os = Get-CimInstance Win32_OperatingSystem
$free = [int64]$os.FreePhysicalMemory * 1KB

Write-Host "==> Ejecutando el corpus con $Proveedor"
$watch = [Diagnostics.Stopwatch]::StartNew()
$p = Start-Process -FilePath 'target\release\nestadmin-predict.exe' -ArgumentList $arguments -PassThru -NoNewWindow
$null = $p.Handle
$peakOllama = 0; $peakClient = 0; $peakTotal = 0
while (-not $p.HasExited) {
    $s = Get-Sample $p.Id
    $peakOllama = [Math]::Max($peakOllama, $s.Ollama)
    $peakClient = [Math]::Max($peakClient, $s.Client)
    $peakTotal = [Math]::Max($peakTotal, $s.Ollama + $s.Client)
    Start-Sleep -Milliseconds 500
}
$p.WaitForExit()
$watch.Stop()
if ($p.ExitCode -ne 0) { throw "nestadmin-predict terminó con código $($p.ExitCode)" }

$report = "t12-$name-$stamp.json"
python evaluation/evaluate.py $out --output $report
if ($LASTEXITCODE -ne 0) { throw 'El evaluador falló' }

$cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1).Name.Trim()
$cv = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
$gb = { param($b) '{0:N2} GB' -f ($b / 1GB) }
$ollamaVersion = try { (Invoke-RestMethod 'http://127.0.0.1:11434/api/version' -TimeoutSec 5).version } catch { 'no responde' }
$summary = @(
    "T12 — $Proveedor$(if ($Modelo) { " ($Modelo)" }) — $(Get-Date -Format 'yyyy-MM-dd HH:mm')",
    "Equipo: $cpu; $(& $gb ($os.TotalVisibleMemorySize * 1KB)) de RAM; $($os.Caption) $($cv.DisplayVersion) ($($os.BuildNumber).$($cv.UBR))",
    "Ollama: $ollamaVersion",
    "Memoria libre al empezar: $(& $gb $free)",
    "Ollama antes de empezar (modelo ya cargado si > 1 GB): $(& $gb $before.Ollama)",
    "Pico de Ollama (suma de procesos ollama*): $(& $gb $peakOllama)",
    "Pico del cliente nestadmin-predict: $(& $gb $peakClient)",
    "Pico conjunto: $(& $gb $peakTotal)",
    "Duración: $([int]$watch.Elapsed.TotalSeconds) s",
    "Predicciones: $out (fuera de Git)",
    "Informe: evaluation/results/$report (fuera de Git)",
    'Método: conjunto de trabajo (WorkingSet64) muestreado cada 500 ms; no incluye memoria compartida de la GPU.'
)
if ($Proveedor -eq 'groq') { $summary += 'Groq: la memoria del servidor remoto no es observable; el pico de Ollama es el de procesos ajenos a la prueba.' }
$summaryPath = "evaluation/results/t12-$name-$stamp.txt"
Set-Content -Path $summaryPath -Value $summary -Encoding UTF8
$summary | ForEach-Object { Write-Host $_ }
Write-Host "==> Resumen en $summaryPath. Copie las cifras a evidence/ (sin las predicciones)."
