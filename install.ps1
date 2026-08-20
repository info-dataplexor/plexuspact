# PlexusPact installer for Windows (x86_64).
#
#   irm https://raw.githubusercontent.com/dataplexor/plexuspact/main/install.ps1 | iex
#
# Environment variables:
#   $env:VERSION      install a specific version (e.g. "0.1.0"); default: latest release
#   $env:INSTALL_DIR  install directory; default: $env:LOCALAPPDATA\Programs\plexuspact
$ErrorActionPreference = "Stop"

$Repo = "dataplexor/plexuspact"
$Bin = "plexuspact"
$Target = "x86_64-pc-windows-msvc"

function Say([string]$msg) { Write-Host "install.ps1: $msg" }
function Fail([string]$msg) { Write-Error "install.ps1: error: $msg"; exit 1 }

if (-not [Environment]::Is64BitOperatingSystem) {
    Fail "prebuilt Windows binaries are x86_64 only; build from source with 'cargo install --git https://github.com/$Repo plexuspact-cli'"
}

# --- Resolve version -----------------------------------------------------------
if ($env:VERSION) {
    $Version = $env:VERSION.TrimStart("v")
} else {
    Say "resolving latest release..."
    $resp = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ "User-Agent" = "plexuspact-install" }
    $Version = ($resp.tag_name).TrimStart("v")
    if (-not $Version) { Fail "could not resolve the latest release tag" }
}
Say "installing $Bin v$Version ($Target)"

# --- Download and verify -------------------------------------------------------
$Archive = "$Bin-v$Version-$Target.zip"
$BaseUrl = "https://github.com/$Repo/releases/download/v$Version"
$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) "plexuspact-install-$([guid]::NewGuid().ToString('n'))"
New-Item -ItemType Directory -Path $Tmp | Out-Null

try {
    Invoke-WebRequest -Uri "$BaseUrl/$Archive" -OutFile (Join-Path $Tmp $Archive) -UseBasicParsing
    Invoke-WebRequest -Uri "$BaseUrl/SHA256SUMS" -OutFile (Join-Path $Tmp "SHA256SUMS") -UseBasicParsing

    $sumLine = Get-Content (Join-Path $Tmp "SHA256SUMS") | Where-Object { $_ -match [regex]::Escape($Archive) + "$" }
    if (-not $sumLine) { Fail "no checksum for $Archive in SHA256SUMS" }
    $Expected = ($sumLine -split "\s+")[0].ToLowerInvariant()
    $Actual = (Get-FileHash -Algorithm SHA256 -Path (Join-Path $Tmp $Archive)).Hash.ToLowerInvariant()
    if ($Expected -ne $Actual) { Fail "checksum mismatch for ${Archive}: expected $Expected, got $Actual" }
    Say "checksum verified"

    Expand-Archive -Path (Join-Path $Tmp $Archive) -DestinationPath $Tmp -Force
    $Exe = Join-Path $Tmp "$Bin.exe"
    if (-not (Test-Path $Exe)) { Fail "archive did not contain $Bin.exe" }

    # --- Install -----------------------------------------------------------------
    if ($env:INSTALL_DIR) { $Dir = $env:INSTALL_DIR } else { $Dir = Join-Path $env:LOCALAPPDATA "Programs\plexuspact" }
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    Copy-Item -Force $Exe (Join-Path $Dir "$Bin.exe")
    Say "installed $(Join-Path $Dir "$Bin.exe")"

    # --- PATH --------------------------------------------------------------------
    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (($UserPath -split ";") -notcontains $Dir) {
        [Environment]::SetEnvironmentVariable("Path", "$UserPath;$Dir", "User")
        $env:Path = "$env:Path;$Dir"
        Say "added $Dir to your user PATH (restart other terminals to pick it up)"
    }

    Say "run '$Bin --version' to verify, then '$Bin init your_data.csv' to get started."
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
