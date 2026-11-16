@echo off
setlocal enabledelayedexpansion
cd /d "%~dp0"

set "REPO=AbobaCDA/kadr"
set "OUT=%USERPROFILE%\Downloads\kadr-build"
set "ACTIONS=https://github.com/AbobaCDA/kadr/actions"

echo.
echo   Kadr - fetching the exe GitHub has already built.
echo   Nothing is compiled here, this only downloads the finished file.
echo.

where gh >nul 2>nul
if errorlevel 1 goto nogh
gh auth status >nul 2>nul
if errorlevel 1 goto noauth

echo   [..] looking for the newest finished build...
set "RUNID="
for /f "usebackq delims=" %%i in (`gh run list -R %REPO% --workflow windows-installer.yml --status success --limit 1 --json databaseId --jq ".[0].databaseId"`) do set "RUNID=%%i"
if "%RUNID%"=="" goto none

echo   [ok] build %RUNID%
echo   [..] downloading...
if exist "%OUT%" rd /s /q "%OUT%"
gh run download %RUNID% -R %REPO% -D "%OUT%"
if errorlevel 1 goto dlfail

set "SETUP="
for /r "%OUT%" %%f in (*setup.exe) do if not defined SETUP set "SETUP=%%f"
set "PORT="
for /r "%OUT%" %%f in (kadr-portable.exe) do if not defined PORT set "PORT=%%f"

echo.
if defined SETUP echo   [ok] installer: !SETUP!
if not defined SETUP echo   [!] no *setup.exe found - the packing step probably failed
if defined PORT  echo   [ok] portable exe: !PORT!
if not defined PORT if not defined SETUP goto empty

echo.
echo   Opening the folder. Run the setup exe, or kadr-portable.exe directly.
start "" explorer "%OUT%"
echo.
pause
exit /b 0

:empty
echo   [X] The build finished but has no exe inside. Open it and check the log:
start "" "%ACTIONS%"
echo.
pause
exit /b 1

:nogh
echo   [!] GitHub CLI (gh) is not installed, so this cannot download for you.
echo.
echo   Take it from the browser instead:
echo     1. Open %ACTIONS%
echo     2. Click the newest green build
echo     3. Scroll to "Artifacts" and download the kadr-windows-... zip
echo     4. Unpack, run the setup exe
start "" "%ACTIONS%"
echo.
pause
exit /b 0

:noauth
echo   [!] gh is installed but not signed in. Run this once:
echo         gh auth login
echo       then start this file again. For now, here is the browser way:
start "" "%ACTIONS%"
echo.
pause
exit /b 0

:none
echo   [!] No successful build yet. One takes about 25 minutes.
echo       Watch the progress here:
start "" "%ACTIONS%"
echo.
pause
exit /b 0

:dlfail
echo.
echo   [X] Download failed: the artifact may have expired (30 days).
echo       Take it from the browser instead:
start "" "%ACTIONS%"
echo.
pause
exit /b 1
