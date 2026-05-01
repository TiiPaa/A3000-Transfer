# Build script — package A3000-Transfer en .exe onedir
# Usage : .\build.ps1
$ErrorActionPreference = "Stop"
Push-Location $PSScriptRoot
try {
    Write-Host "Cleaning build/ and dist/..."
    Remove-Item -Recurse -Force build, dist -ErrorAction SilentlyContinue

    Write-Host "Running PyInstaller..."
    pyinstaller --noconfirm a3000_transfer.spec

    Write-Host "Applying scipy post-build patch..."
    python patch_scipy.py

    if (Test-Path dist\A3000Transfer\A3000Transfer.exe) {
        Write-Host ""
        Write-Host "Build done: $PSScriptRoot\dist\A3000Transfer\A3000Transfer.exe"
        Write-Host "To distribute, zip the entire 'dist\A3000Transfer\' folder."
    } else {
        Write-Host "Build FAILED: A3000Transfer.exe not found in dist\." -ForegroundColor Red
        exit 1
    }
} finally {
    Pop-Location
}
