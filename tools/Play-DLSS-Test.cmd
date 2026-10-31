@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0play_dlss_test.ps1" %*
if errorlevel 1 pause
