@echo off
setlocal
cd /d "%~dp0"

echo.
echo   Kadr - publishing the source to GitHub.
echo   The installer itself is built later by GitHub Actions (20 min).
echo.

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0tools\publish.ps1" %*
set "RC=%ERRORLEVEL%"

echo.
if not "%RC%"=="0" (
    echo   The script stopped with code %RC%. See the message above.
) else (
    echo   Done. Open the Actions link printed above and press "Run workflow".
)
echo.
pause
