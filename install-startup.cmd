<# : batch portion
@echo off & setlocal
set "SCRIPT_PATH=%~f0"
powershell -NoProfile -ExecutionPolicy Bypass -Command "iex ((Get-Content -LiteralPath $env:SCRIPT_PATH) -join [char]10)"
if %errorlevel% neq 0 (
    echo.
    echo Script failed with error level %errorlevel%.
    pause
) else (
    echo.
    echo Script completed successfully.
    pause
)
exit /b %errorlevel%
#>

# 시작 프로그램 등록 스크립트 — 작업 스케줄러 버전 (설계 §14, §16.2)
#
# 릴리스 빌드는 관리자 권한(requireAdministrator)으로 실행되므로, Startup 폴더 바로가기로
# 자동 시작하면 로그온마다 UAC 동의창이 뜬다. 이를 피하려고 "로그온 시 + 가장 높은 권한"
# 작업으로 등록한다 → 다음 로그온부터 UAC 없이 관리자 권한으로 자동 시작.
#
# 작업 등록 자체에는 관리자 권한이 필요하므로, 관리자가 아니면 UAC 로 한 번 재실행한다(설치 시 1회).

$ErrorActionPreference = 'Stop'

# $SCRIPT_PATH는 배치 파일 환경 변수에서 가져온 원래 경로입니다.
# 이를 통해 $PSScriptRoot를 정의하여 하위 호환성을 유지합니다.
$PSScriptRoot = Split-Path -Parent $env:SCRIPT_PATH

# --- 관리자 권한 확인 + 필요 시 자기 자신을 관리자로 재실행 ---
$IsAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $IsAdmin) {
    Write-Host "Task Scheduler registration requires administrator privileges. Prompting for elevation (UAC)..."
    try {
        Start-Process -FilePath "cmd.exe" -ArgumentList "/c `"$env:SCRIPT_PATH`"" -Verb RunAs
    } catch {
        Write-Error "Elevation was cancelled."
        exit 1
    }
    exit
}

# --- 실행 파일 경로 결정 ---
# 스크립트와 같은 폴더의 caps-hangul.exe 를 우선 찾고, 없으면 release 빌드 산출물 경로를 시도한다.
# 빌드는 x64 / aarch64 두 타겟을 모두 생성하므로(.cargo/config.toml 참조),
# 현재 PC 아키텍처에 맞는 산출물을 먼저 고른다.
$ExeName = 'caps-hangul.exe'

$Arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
switch ($Arch) {
    'Arm64' { $Triple = 'aarch64-pc-windows-msvc' }
    default { $Triple = 'x86_64-pc-windows-msvc' }   # X64
}

$Candidates = @(
    (Join-Path $PSScriptRoot $ExeName),
    (Join-Path $PSScriptRoot "target\$Triple\release\$ExeName"),
    # 다른 아키텍처 산출물 / 단일 타겟 빌드 호환용 폴백
    (Join-Path $PSScriptRoot "target\x86_64-pc-windows-msvc\release\$ExeName"),
    (Join-Path $PSScriptRoot "target\aarch64-pc-windows-msvc\release\$ExeName"),
    (Join-Path $PSScriptRoot "target\release\$ExeName")
)
$TargetPath = $Candidates | Where-Object { Test-Path $_ } | Select-Object -First 1

if (-not $TargetPath) {
    Write-Error "$ExeName not found. Build it first with '.\build.ps1', or place the exe in this folder."
    exit 1
}
$ExeDir = Split-Path $TargetPath -Parent

# 주입용 TSF 리더 DLL 이 exe 옆에 있는지 확인한다(없어도 동작은 하지만 한/영 라벨이 추정값
# 폴백으로만 동작 — Teams 등 TSF/Chromium 앱에서 부정확). 배포본: caps-hangul-tsf-<arch>.dll,
# 개발(cargo) 빌드: caps_hangul_tsf.dll.
switch ($Arch) { 'Arm64' { $DllArch = 'arm64' } default { $DllArch = 'x64' } }
$DllCandidates = @("caps-hangul-tsf-$DllArch.dll", 'caps_hangul_tsf.dll')
if (-not ($DllCandidates | Where-Object { Test-Path (Join-Path $ExeDir $_) })) {
    Write-Warning "TSF reader DLL ($($DllCandidates -join ' / ')) not found next to the exe. The Han/Eng label will fall back to a best-guess value. Build it together with '.\build.ps1'."
}

# --- 작업 스케줄러 등록 ---
$TaskName = 'Caps Hangul'
$User = "$env:USERDOMAIN\$env:USERNAME"

$Action  = New-ScheduledTaskAction -Execute $TargetPath -WorkingDirectory $ExeDir
$Trigger = New-ScheduledTaskTrigger -AtLogOn -User $User
# 현재 사용자의 대화형 세션에서 "가장 높은 권한"으로 실행 → 로그온 시 UAC 없이 관리자 권한 시작.
$Principal = New-ScheduledTaskPrincipal -UserId $User -LogonType Interactive -RunLevel Highest
$Settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -StartWhenAvailable `
    -MultipleInstances IgnoreNew `
    -ExecutionTimeLimit ([TimeSpan]::Zero)   # 시간 제한 없음(상주 프로그램)

Register-ScheduledTask -TaskName $TaskName `
    -Action $Action -Trigger $Trigger -Principal $Principal -Settings $Settings `
    -Description 'Caps Lock 한/영 전환 유틸리티 (로그온 시 관리자 권한으로 자동 시작)' `
    -Force | Out-Null

# 구버전 Startup 폴더 바로가기가 있으면 정리한다(작업 스케줄러와 중복 실행 방지).
$LegacyLnk = Join-Path ([Environment]::GetFolderPath('Startup')) 'Caps Hangul.lnk'
if (Test-Path $LegacyLnk) {
    Remove-Item $LegacyLnk -Force
    Write-Host "Removed legacy Startup shortcut: $LegacyLnk"
}

Write-Host ""
Write-Host "Registered Task Scheduler task '$TaskName'"
Write-Host "  Run     : $TargetPath"
Write-Host "  Trigger : At logon (user $User), highest privileges"
Write-Host "  -> Will auto-start with administrator privileges (no UAC) from the next logon."
Write-Host ""
Write-Host "Starting the task now..."
Start-ScheduledTask -TaskName $TaskName
Write-Host "The application is now running in the background."
Write-Host ""
