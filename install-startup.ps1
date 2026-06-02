# 시작 프로그램 등록 스크립트 (설계 §14.2)
# 현재 사용자 Startup 폴더에 caps-hangul.exe 바로가기를 생성한다. 관리자 권한 불필요.

$ErrorActionPreference = 'Stop'

# 실행 파일 경로: 스크립트와 같은 폴더의 caps-hangul.exe 를 우선 찾고,
# 없으면 release 빌드 산출물 경로를 시도한다.
# 빌드는 x64 / aarch64 두 타겟을 모두 생성하므로(.cargo/config.toml 참조),
# 현재 PC 아키텍처에 맞는 산출물을 먼저 고른다.
$ExeName = 'caps-hangul.exe'

# 현재 OS 아키텍처에 대응하는 rustc target triple
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
    Write-Error "$ExeName 을(를) 찾을 수 없습니다. 먼저 '.\build.ps1' 로 빌드하거나 exe 를 이 폴더에 두세요."
    exit 1
}

# 주입용 TSF 리더 DLL 이 exe 옆에 있는지 확인한다(없어도 동작은 하지만 한/영 라벨이 추정값
# 폴백으로만 동작 — Teams 등 TSF/Chromium 앱에서 부정확). 배포본: caps-hangul-tsf-<arch>.dll,
# 개발(cargo) 빌드: caps_hangul_tsf.dll.
$ExeDir = Split-Path $TargetPath -Parent
switch ($Arch) { 'Arm64' { $DllArch = 'arm64' } default { $DllArch = 'x64' } }
$DllCandidates = @("caps-hangul-tsf-$DllArch.dll", 'caps_hangul_tsf.dll')
if (-not ($DllCandidates | Where-Object { Test-Path (Join-Path $ExeDir $_) })) {
    Write-Warning "TSF 리더 DLL($($DllCandidates -join ' / '))이 exe 옆에 없습니다. 한/영 라벨이 추정값 폴백으로 동작합니다. '.\build.ps1' 로 함께 빌드하세요."
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
