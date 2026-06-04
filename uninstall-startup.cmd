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

# 시작 프로그램 해제 스크립트 — 작업 스케줄러 버전 (설계 §14, §16.2)
# install-startup.ps1 이 등록한 "Caps Hangul" 작업을 제거한다.
# 가장 높은 권한 작업의 해제에는 관리자 권한이 필요하므로, 관리자가 아니면 UAC 로 한 번 재실행한다.

$ErrorActionPreference = 'Stop'

# $SCRIPT_PATH는 배치 파일 환경 변수에서 가져온 원래 경로입니다.
# 이를 통해 $PSScriptRoot를 정의하여 하위 호환성을 유지합니다.
$PSScriptRoot = Split-Path -Parent $env:SCRIPT_PATH

# --- 관리자 권한 확인 + 필요 시 자기 자신을 관리자로 재실행 ---
$IsAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $IsAdmin) {
    Write-Host "Task Scheduler removal requires administrator privileges. Prompting for elevation (UAC)..."
    try {
        Start-Process -FilePath "cmd.exe" -ArgumentList "/c `"$env:SCRIPT_PATH`"" -Verb RunAs
    } catch {
        Write-Error "Elevation was cancelled."
        exit 1
    }
    exit
}

$TaskName = 'Caps Hangul'

$Task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($Task) {
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
    Write-Host "Removed Task Scheduler task '$TaskName'"
} else {
    Write-Host "No registered task found: '$TaskName'"
}
