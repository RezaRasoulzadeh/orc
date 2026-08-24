$ErrorActionPreference = "Stop"
$ProjectDir = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$InstallDir = Join-Path $env:LOCALAPPDATA "Programs\Orc"

if ($args.Count -gt 0 -and $args[0] -eq "--uninstall") {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $InstallDir
    exit 0
}

Set-Location $ProjectDir
npm ci
npm run typecheck
npm run build
cargo build --release --bin orc
npm run tauri:build
npm run validate:package
New-Item -ItemType Directory -Force $InstallDir | Out-Null
Copy-Item target\release\orc.exe (Join-Path $InstallDir "orc.exe") -Force
Copy-Item src-tauri\target\release\orc-desktop.exe (Join-Path $InstallDir "orc-desktop.exe") -Force
$Path = [Environment]::GetEnvironmentVariable("Path", "User")
if (($Path -split ";") -notcontains $InstallDir) {
    [Environment]::SetEnvironmentVariable("Path", "$Path;$InstallDir", "User")
}
