$ErrorActionPreference = "Stop"

$repo = "mbcat456/ulp"
$asset = "ulp-windows-x86_64.zip"
$dest = if ($env:ULP_INSTALL_DIR) {
    $env:ULP_INSTALL_DIR
} else {
    Join-Path $env:LOCALAPPDATA "Programs\ulp"
}
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("ulp-" + [guid]::NewGuid().ToString("N"))

New-Item -ItemType Directory -Force -Path $tmp | Out-Null
try {
    $zip = Join-Path $tmp $asset
    Invoke-WebRequest "https://github.com/$repo/releases/latest/download/$asset" -OutFile $zip
    New-Item -ItemType Directory -Force -Path $dest | Out-Null
    Expand-Archive -LiteralPath $zip -DestinationPath $dest -Force
    Write-Host "Installed ulp to $dest\ulp.exe"
} finally {
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
