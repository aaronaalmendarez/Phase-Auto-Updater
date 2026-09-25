@echo off
call "%~dp0msvc-env-x64.cmd" || exit /b %ERRORLEVEL%
"%MSVC_LIB%" %*
exit /b %ERRORLEVEL%
