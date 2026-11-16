@echo off
setlocal
cd /d "%~dp0"

echo.
echo   Kadr - publish the update to GitHub.
echo.

if not exist ".git" (
    echo   [X] This folder has no .git - extract the folder that already
    echo       contains your repository, not a fresh copy.
    echo.
    pause
    exit /b 1
)

git add -A
if errorlevel 1 goto fail
git commit -q -m "ci: retry NSIS bundling, keep the portable exe, no PR builds"
echo   [ok] committed (if there was anything new)

echo   [..] pushing to GitHub...
git push origin master
if errorlevel 1 goto pushfail

echo.
echo   [ok] pushed. The Windows build starts on its own.
echo        https://github.com/AbobaCDA/kadr/actions
echo.
pause
exit /b 0

:pushfail
echo.
echo   [X] Push failed. Usual reasons:
echo       - you are not signed in to GitHub: run  gh auth login
echo         or let the browser window finish signing you in
echo       - the repository is behind: run  git pull --rebase origin master
echo         and start this file again
echo.
pause
exit /b 1

:fail
echo.
echo   [X] git add failed - see the message above.
echo.
pause
exit /b 1
