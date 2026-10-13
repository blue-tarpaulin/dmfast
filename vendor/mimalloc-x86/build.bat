@echo off
rem Rebuilds mimalloc.dll here: hybrid.c linked with mimalloc v3.5.4 (static, x86, no override).
rem MI_OPT_ARCH=OFF: CMake mistakes this x86 build for x64 and would otherwise enable AVX2.
rem Needs git, CMake and the Visual Studio x86 build tools.
setlocal
set VCVARS=C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvarsall.bat
if not exist "%VCVARS%" for /f "delims=" %%i in ('"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -find VC\Auxiliary\Build\vcvarsall.bat') do set VCVARS=%%i
call "%VCVARS%" x86 >nul || exit /b 1
cd /d %~dp0
set SRC=%TEMP%\dmfast-mimalloc-3.5.4
if not exist "%SRC%" git clone -q --depth 1 --branch v3.5.4 https://github.com/microsoft/mimalloc.git "%SRC%" || exit /b 1
if exist "%SRC%\build32" rmdir /s /q "%SRC%\build32"
cmake -S "%SRC%" -B "%SRC%\build32" -G "NMake Makefiles" -DCMAKE_BUILD_TYPE=Release -DMI_BUILD_TESTS=OFF
  -DMI_BUILD_SHARED=OFF -DMI_BUILD_STATIC=ON -DMI_BUILD_OBJECT=OFF -DMI_OVERRIDE=OFF -DMI_WIN_REDIRECT=OFF
  -DCMAKE_SYSTEM_PROCESSOR=x86 -DMI_OPT_ARCH=OFF || exit /b 1
cmake --build "%SRC%\build32" || exit /b 1
set OBJ=%TEMP%\dmfast-hybrid-obj
if not exist "%OBJ%" mkdir "%OBJ%"
cl /nologo /O2 /W4 /MD /LD /I "%SRC%\include" hybrid.c /Fe:mimalloc.dll /Fo:"%OBJ%\\" /link /NOLOGO /IMPLIB:"%OBJ%\mimalloc.lib"
  "%SRC%\build32\mimalloc.lib" advapi32.lib bcrypt.lib psapi.lib /EXPORT:mi_version /EXPORT:mi_is_redirected=hybrid_active || exit /b 1
