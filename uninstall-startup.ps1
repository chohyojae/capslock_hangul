# 시작 프로그램 해제 스크립트 (설계 §14)
# 현재 사용자 Startup 폴더에서 caps-hangul 바로가기를 제거한다.

$ErrorActionPreference = 'Stop'

$Startup = [Environment]::GetFolderPath('Startup')
$ShortcutPath = Join-Path $Startup 'Caps Hangul.lnk'

if (Test-Path $ShortcutPath) {
    Remove-Item $ShortcutPath -Force
    Write-Host "해제 완료: $ShortcutPath"
} else {
    Write-Host "등록된 바로가기가 없습니다: $ShortcutPath"
}
