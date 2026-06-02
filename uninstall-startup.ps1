# 시작 프로그램 해제 스크립트 — 작업 스케줄러 버전 (설계 §14, §16.2)
# install-startup.ps1 이 등록한 "Caps Hangul" 작업을 제거한다.
# 가장 높은 권한 작업의 해제에는 관리자 권한이 필요하므로, 관리자가 아니면 UAC 로 한 번 재실행한다.

$ErrorActionPreference = 'Stop'

# --- 관리자 권한 확인 + 필요 시 자기 자신을 관리자로 재실행 ---
$IsAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $IsAdmin) {
    Write-Host "Task Scheduler removal requires administrator privileges. Prompting for elevation (UAC)..."
    $HostPath = (Get-Process -Id $PID).Path
    $ReArgs = "-NoProfile -ExecutionPolicy Bypass -NoExit -File `"$PSCommandPath`""
    try {
        Start-Process -FilePath $HostPath -Verb RunAs -ArgumentList $ReArgs
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

# 구버전 Startup 폴더 바로가기도 함께 정리한다(이전 버전에서 등록했을 수 있음).
$LegacyLnk = Join-Path ([Environment]::GetFolderPath('Startup')) 'Caps Hangul.lnk'
if (Test-Path $LegacyLnk) {
    Remove-Item $LegacyLnk -Force
    Write-Host "Removed legacy Startup shortcut: $LegacyLnk"
}
