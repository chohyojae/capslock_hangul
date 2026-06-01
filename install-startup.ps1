# 시작 프로그램 등록 스크립트 (설계 §14.2)
# 현재 사용자 Startup 폴더에 caps-hangul.exe 바로가기를 생성한다. 관리자 권한 불필요.

$ErrorActionPreference = 'Stop'

# 실행 파일 경로: 스크립트와 같은 폴더의 caps-hangul.exe 를 우선 찾고,
# 없으면 release 빌드 산출물 경로를 시도한다.
$ExeName = 'caps-hangul.exe'
$Candidates = @(
    (Join-Path $PSScriptRoot $ExeName),
    (Join-Path $PSScriptRoot "target\release\$ExeName")
)
$TargetPath = $Candidates | Where-Object { Test-Path $_ } | Select-Object -First 1

if (-not $TargetPath) {
    Write-Error "$ExeName 을(를) 찾을 수 없습니다. 먼저 'cargo build --release' 로 빌드하거나 exe 를 이 폴더에 두세요."
    exit 1
}

$Startup = [Environment]::GetFolderPath('Startup')
$ShortcutPath = Join-Path $Startup 'Caps Hangul.lnk'

$WshShell = New-Object -ComObject WScript.Shell
$Shortcut = $WshShell.CreateShortcut($ShortcutPath)
$Shortcut.TargetPath = $TargetPath
$Shortcut.WorkingDirectory = Split-Path $TargetPath -Parent
$Shortcut.Description = 'Caps Lock 한/영 전환 유틸리티'
$Shortcut.Save()

Write-Host "등록 완료: $ShortcutPath"
Write-Host "  -> $TargetPath"
