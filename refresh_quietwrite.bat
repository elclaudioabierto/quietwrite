@echo off
setlocal

set "DISTRO=Ubuntu"
set "PROJECT=/home/nelson/.openclaw/workspace/projects/quietwrite"
set "WSL_PROJECT=\\wsl.localhost\Ubuntu\home\nelson\.openclaw\workspace\projects\quietwrite"
set "WSL_RELEASE=%WSL_PROJECT%\target\arm-unknown-linux-musleabihf\release"
set "REMOTE=zen@zen.local"
set "REMOTE_STAGE=/home/zen/quietwrite-refresh"

echo [1/4] Building and testing the latest ARMv6 release...
wsl.exe -d "%DISTRO%" --cd "%PROJECT%" sh ./scripts/build-pi-armv6.sh
if errorlevel 1 goto :fail

echo [2/4] Preparing the staging directory on the Pi...
ssh "%REMOTE%" "mkdir -p %REMOTE_STAGE%"
if errorlevel 1 goto :fail

echo [3/4] Transferring the binary, checksum, and refresh helper...
scp "%WSL_RELEASE%\quietwrite" "%WSL_RELEASE%\quietwrite.sha256" "%WSL_PROJECT%\scripts\refresh-pi-remote.sh" "%REMOTE%:%REMOTE_STAGE%/"
if errorlevel 1 goto :fail

echo [4/4] Verifying, installing, and restarting QuietWrite...
ssh -t "%REMOTE%" "sh %REMOTE_STAGE%/refresh-pi-remote.sh"
if errorlevel 1 goto :fail

echo.
echo QuietWrite refresh completed successfully.
exit /b 0

:fail
echo.
echo QuietWrite refresh failed. Review the error above; the Pi installation was left unchanged or rolled back.
exit /b 1
