@echo off
set "MSVC_ROOT="

for /d %%V in ("%ProgramFiles%\Microsoft Visual Studio\*") do (
  for /d %%I in ("%%~fV\*") do (
    for /d %%M in ("%%~fI\VC\Tools\MSVC\*") do (
      if not defined MSVC_ROOT if exist "%%~fM\include\vcruntime.h" if exist "%%~fM\lib\x64\libcmt.lib" if exist "%%~fM\bin\HostX64\x64\cl.exe" set "MSVC_ROOT=%%~fM"
    )
  )
)

for /d %%V in ("%ProgramFiles(x86)%\Microsoft Visual Studio\*") do (
  for /d %%I in ("%%~fV\*") do (
    for /d %%M in ("%%~fI\VC\Tools\MSVC\*") do (
      if not defined MSVC_ROOT if exist "%%~fM\include\vcruntime.h" if exist "%%~fM\lib\x64\libcmt.lib" if exist "%%~fM\bin\HostX64\x64\cl.exe" set "MSVC_ROOT=%%~fM"
    )
  )
)

if not defined MSVC_ROOT (
  echo Could not find a complete x64 MSVC toolset with vcruntime.h, libcmt.lib, and cl.exe. 1>&2
  exit /b 1
)

set "WINDOWS_SDK_ROOT=%ProgramFiles(x86)%\Windows Kits\10"
set "WINDOWS_SDK_VERSION="
for /d %%S in ("%WINDOWS_SDK_ROOT%\Include\10.*") do (
  if exist "%WINDOWS_SDK_ROOT%\Lib\%%~nxS\ucrt\x64" set "WINDOWS_SDK_VERSION=%%~nxS"
)

if not defined WINDOWS_SDK_VERSION (
  echo Could not find a usable Windows SDK 10 include/lib version. 1>&2
  exit /b 1
)

set "MSVC_BIN=%MSVC_ROOT%\bin\HostX64\x64"
set "MSVC_CL=%MSVC_BIN%\cl.exe"
set "MSVC_LIB=%MSVC_BIN%\lib.exe"
set "MSVC_LINK=%MSVC_BIN%\link.exe"
set "PATH=%MSVC_BIN%;%PATH%"
set "INCLUDE=%MSVC_ROOT%\include;%WINDOWS_SDK_ROOT%\Include\%WINDOWS_SDK_VERSION%\ucrt;%WINDOWS_SDK_ROOT%\Include\%WINDOWS_SDK_VERSION%\shared;%WINDOWS_SDK_ROOT%\Include\%WINDOWS_SDK_VERSION%\um;%WINDOWS_SDK_ROOT%\Include\%WINDOWS_SDK_VERSION%\winrt;%WINDOWS_SDK_ROOT%\Include\%WINDOWS_SDK_VERSION%\cppwinrt;%INCLUDE%"
set "LIB=%MSVC_ROOT%\lib\x64;%WINDOWS_SDK_ROOT%\Lib\%WINDOWS_SDK_VERSION%\ucrt\x64;%WINDOWS_SDK_ROOT%\Lib\%WINDOWS_SDK_VERSION%\um\x64;%LIB%"

exit /b 0
