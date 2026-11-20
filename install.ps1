# buscador para Windows: descarga el instalador de la última versión y lo abre.
#
#   irm https://raw.githubusercontent.com/airylp/buscador/main/install.ps1 | iex
#
# El instalador lo crea .github/workflows/windows.yml en GitHub Releases.
# Descargado así no lleva la marca «de Internet», así que Windows no muestra
# el aviso de SmartScreen.

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # Invoke-WebRequest va mucho más rápido sin barra
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$repo = if ($env:BUSCADOR_REPO) { $env:BUSCADOR_REPO } else { 'airylp/buscador' }
$lang = (Get-UICulture).TwoLetterISOLanguageName
function m($es, $en, $fr) { switch ($lang) { 'es' { $es } 'fr' { $fr } default { $en } } }

Write-Host (m '==> Buscando la última versión de buscador...' '==> Looking for the latest version of buscador...' '==> Recherche de la dernière version de buscador...')
$rel = Invoke-RestMethod -UseBasicParsing "https://api.github.com/repos/$repo/releases/latest"
$asset = $rel.assets | Where-Object { $_.name -like '*setup*.exe' } | Select-Object -First 1
if (-not $asset) {
    throw (m "La versión $($rel.tag_name) no tiene instalador para Windows." "Version $($rel.tag_name) has no Windows installer." "La version $($rel.tag_name) n'a pas d'installateur Windows.")
}

$file = Join-Path $env:TEMP $asset.name
Write-Host (m "==> Descargando $($asset.name)..." "==> Downloading $($asset.name)..." "==> Téléchargement de $($asset.name)...")
Invoke-WebRequest -UseBasicParsing $asset.browser_download_url -OutFile $file

Write-Host (m '==> Abriendo el instalador...' '==> Opening the installer...' "==> Ouverture de l'installateur...")
try {
    $p = Start-Process -FilePath $file -PassThru -Wait
    if ($p.ExitCode -eq 0) {
        Write-Host (m "==> buscador $($rel.tag_name) instalado." "==> buscador $($rel.tag_name) installed." "==> buscador $($rel.tag_name) installé.")
    }
} finally {
    Remove-Item $file -ErrorAction SilentlyContinue
}
