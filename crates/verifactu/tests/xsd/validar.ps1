# Valida un XML contra los XSD de esta carpeta con System.Xml de .NET.
# Uso: powershell -File validar.ps1 <carpeta-xsd> <documento.xml>
# Escribe VALIDO o la lista de errores (y sale con 1).
param([Parameter(Mandatory)][string]$Dir, [Parameter(Mandatory)][string]$Xml)
$ErrorActionPreference = 'Stop'
# SuministroInformacion.xsd declara UTF-8 pero trae comentarios en Latin-1; leerlo como
# Latin-1 solo altera esos comentarios. Los imports no se resuelven por red: todos los
# espacios de nombres se añaden al conjunto desde esta carpeta.
$latin1 = [System.Text.Encoding]::GetEncoding(28591)
$set = New-Object System.Xml.Schema.XmlSchemaSet
$set.XmlResolver = $null
foreach ($f in 'xmldsig-core-schema.xsd', 'SuministroInformacion.xsd', 'SuministroLR.xsd', 'RespuestaSuministro.xsd') {
    $rs = New-Object System.Xml.XmlReaderSettings
    $rs.DtdProcessing = [System.Xml.DtdProcessing]::Parse
    $rs.XmlResolver = $null
    $sr = New-Object System.IO.StreamReader((Join-Path $Dir $f), $latin1)
    $r = [System.Xml.XmlReader]::Create($sr, $rs)
    [void]$set.Add([System.Xml.Schema.XmlSchema]::Read($r, $null))
    $r.Close()
}
$set.Compile()
$errores = New-Object System.Collections.ArrayList
$vs = New-Object System.Xml.XmlReaderSettings
$vs.ValidationType = [System.Xml.ValidationType]::Schema
$vs.Schemas = $set
$vs.ValidationFlags = $vs.ValidationFlags -bor [System.Xml.Schema.XmlSchemaValidationFlags]::ReportValidationWarnings
$vs.add_ValidationEventHandler({ param($s, $e) [void]$errores.Add("$($e.Severity): $($e.Message) (linea $($e.Exception.LineNumber))") })
$r = [System.Xml.XmlReader]::Create($Xml, $vs)
while ($r.Read()) { }
$r.Close()
if ($errores.Count) { $errores | ForEach-Object { Write-Output $_ }; exit 1 }
Write-Output 'VALIDO'
