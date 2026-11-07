# Descarga los XSD oficiales que usan las pruebas `#[ignore]` de tests/xml.rs.
# No se versionan: el aviso legal de la AEAT no autoriza su redistribución (ver README.md).
# Compara el SHA-256 con el verificado (FUENTES.md); si la AEAT publica otra versión,
# avisa sin borrar el archivo para que se revise el cambio antes de actualizar el crate.
$ErrorActionPreference = 'Stop'
$dir = $PSScriptRoot
$aeat = 'https://www2.agenciatributaria.gob.es/static_files/common/internet/dep/aplicaciones/es/aeat/tike/cont/ws'
$archivos = [ordered]@{
    'SuministroLR.xsd'          = @("$aeat/SuministroLR.xsd", 'cbdac8d427cc5ab5d77ca48974cab0f35d6bb819c4c66db361681e3710aeba36')
    'SuministroInformacion.xsd' = @("$aeat/SuministroInformacion.xsd", 'ee4c1655175644de44c4c25055ffeb8e5f4bb4bc3834ce8254d4222ef18c8aa1')
    'RespuestaSuministro.xsd'   = @("$aeat/RespuestaSuministro.xsd", '82acf80f785643caac13087aae66808ed721a13f08ca5218cf8ae81b695549ef')
    'xmldsig-core-schema.xsd'   = @('https://www.w3.org/TR/xmldsig-core/xmldsig-core-schema.xsd', 'd102ad3df7664c307e0c2c776ba4a90513b1969974d8a940bae1a77f9f21e15d')
}
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$distintos = 0
foreach ($nombre in $archivos.Keys) {
    $url, $esperado = $archivos[$nombre]
    $destino = Join-Path $dir $nombre
    Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $destino
    $hash = (Get-FileHash -Algorithm SHA256 $destino).Hash.ToLowerInvariant()
    if ($hash -eq $esperado) {
        Write-Output "OK  $nombre"
    } else {
        Write-Warning "$nombre ha cambiado (SHA-256 $hash). Revise FUENTES.md y la nueva versión."
        $distintos++
    }
}
if ($distintos) { exit 2 }
