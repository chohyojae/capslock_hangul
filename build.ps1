#Requires -Version 5.1
<#
.SYNOPSIS
  caps-hangul-rs 릴리스 빌드 + 배포 패키지 생성.

.DESCRIPTION
  주입 컴포넌트(TSF 리더 DLL + per-arch 헬퍼 exe)를 x86/x64/arm64 **세 비트니스 모두** 빌드하고,
  본체 exe(caps-hangul.exe)는 패키지 대상 비트니스로 빌드한다. 그런 다음 본체 옆에 세 비트니스의
  DLL·헬퍼를 모두 동봉한 자기완결 배포 폴더를 dist\ 아래에 만든다.

  왜 3종을 모두 동봉하나:
    SetWindowsHookEx 는 호출자·주입 DLL·대상 프로세스의 비트니스가 모두 같아야 한다(README 참조).
    포커스 프로세스가 x86/x64/arm64 중 무엇일지 미리 알 수 없으므로, 본체는 같은-비트니스 포커스는
    직접(in-process) 읽고, 다른-비트니스 포커스는 그 비트니스의 헬퍼(caps-hangul-reader-<arch>.exe)에
    주입을 위임한다. 따라서 세 비트니스의 DLL·헬퍼가 모두 필요하다.

  배포 폴더 구성(예: x64 본체):
    dist\caps-hangul-x64\
      caps-hangul.exe                  (본체, x64)
      caps-hangul-tsf-x86.dll          \
      caps-hangul-tsf-x64.dll           > 세 비트니스 리더 DLL
      caps-hangul-tsf-arm64.dll        /
      caps-hangul-reader-x86.exe       \
      caps-hangul-reader-x64.exe        > 세 비트니스 주입 헬퍼
      caps-hangul-reader-arm64.exe     /
      install-startup.ps1
      uninstall-startup.ps1
      README.md
      LICENSE

.PARAMETER Arch
  본체 exe(패키지) 대상. x64 | arm64 | all. 미지정 시 현재 PC 아키텍처.
  (컴포넌트 DLL·헬퍼는 이 값과 무관하게 항상 x86/x64/arm64 세 종 모두 빌드한다.)

.PARAMETER Zip
  지정 시 각 배포 폴더를 dist\caps-hangul-<arch>.zip 으로 압축한다.

.EXAMPLE
  .\build.ps1
  현재 아키텍처(보통 x64) 본체 패키지 1개.

.EXAMPLE
  .\build.ps1 -Arch all -Zip
  x64 / arm64 본체 패키지 모두 빌드 후 zip 압축.

.NOTES
  선행 조건(세 비트니스 컴포넌트 빌드용):
    rustup target add i686-pc-windows-msvc x86_64-pc-windows-msvc aarch64-pc-windows-msvc
  - i686 cross-link 에는 VS "MSVC v143 - x86/x64 빌드 도구"(보통 기본 포함)가 필요하다.
  - aarch64 cross-link 에는 VS "MSVC v143 - ARM64 빌드 도구" 컴포넌트가 필요하다.
#>
[CmdletBinding()]
param(
    [ValidateSet('x64', 'arm64', 'all')]
    [string]$Arch,
    [switch]$Zip
)

$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

# 아키텍처 접미사 → rustc target triple
$Triple = @{
    'x86'   = 'i686-pc-windows-msvc'
    'x64'   = 'x86_64-pc-windows-msvc'
    'arm64' = 'aarch64-pc-windows-msvc'
}

# 컴포넌트(DLL + 헬퍼)는 대상이 어느 비트니스든 대응하도록 항상 세 종 모두 빌드한다.
$Components = @('x86', 'x64', 'arm64')

# 본체 exe 패키지 대상(미지정 시 현재 PC 아키텍처).
if (-not $Arch) {
    $os = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    $Arch = if ($os -eq 'Arm64') { 'arm64' } else { 'x64' }
    Write-Host "본체 패키지 대상 미지정 → 현재 PC 아키텍처 '$Arch' 사용" -ForegroundColor DarkGray
}
$MainArches = if ($Arch -eq 'all') { @('x64', 'arm64') } else { @($Arch) }

function Invoke-Cargo {
    param([string[]]$CargoArgs, [string]$What)
    Write-Host "==> cargo $($CargoArgs -join ' ')" -ForegroundColor Cyan
    & cargo @CargoArgs
    if ($LASTEXITCODE -ne 0) {
        throw "$What 빌드 실패 (exit $LASTEXITCODE). 'rustup target add <triple>' 와 해당 VS 빌드 도구 설치를 확인하세요."
    }
}

# 1) 컴포넌트(리더 DLL + 헬퍼) 세 비트니스 빌드.
foreach ($c in $Components) {
    Invoke-Cargo @('build', '--release', '-p', 'caps-hangul-tsf', '-p', 'caps-hangul-reader', '--target', $Triple[$c]) "[$c] 컴포넌트"
}

# 2) 본체 exe(패키지 대상 비트니스) 빌드.
foreach ($m in $MainArches) {
    Invoke-Cargo @('build', '--release', '-p', 'caps-hangul-rs', '--target', $Triple[$m]) "[$m] 본체 exe"
}

# 3) 패키지 조립.
$DistRoot = Join-Path $PSScriptRoot 'dist'
$null = New-Item -ItemType Directory -Path $DistRoot -Force

function Copy-Checked {
    param([string]$Src, [string]$Dst)
    if (-not (Test-Path $Src)) { throw "산출물 누락: $Src" }
    Copy-Item $Src $Dst
}

foreach ($m in $MainArches) {
    $pkgName = "caps-hangul-$m"
    $pkgDir = Join-Path $DistRoot $pkgName
    if (Test-Path $pkgDir) { Remove-Item $pkgDir -Recurse -Force }
    $null = New-Item -ItemType Directory -Path $pkgDir -Force

    # 본체 exe
    $mainRel = Join-Path $PSScriptRoot "target\$($Triple[$m])\release"
    Copy-Checked (Join-Path $mainRel 'caps-hangul.exe') (Join-Path $pkgDir 'caps-hangul.exe')

    # 세 비트니스 DLL + 헬퍼(아키텍처 접미사로 리네이밍)
    foreach ($c in $Components) {
        $rel = Join-Path $PSScriptRoot "target\$($Triple[$c])\release"
        Copy-Checked (Join-Path $rel 'caps_hangul_tsf.dll')   (Join-Path $pkgDir "caps-hangul-tsf-$c.dll")
        Copy-Checked (Join-Path $rel 'caps-hangul-reader.exe') (Join-Path $pkgDir "caps-hangul-reader-$c.exe")
    }

    # 설치 스크립트 + 문서
    foreach ($extra in 'install-startup.ps1', 'uninstall-startup.ps1', 'README.md', 'LICENSE') {
        $src = Join-Path $PSScriptRoot $extra
        if (Test-Path $src) { Copy-Item $src (Join-Path $pkgDir $extra) }
    }

    Write-Host "    패키지: $pkgDir" -ForegroundColor Green

    if ($Zip) {
        $zipPath = Join-Path $DistRoot "$pkgName.zip"
        if (Test-Path $zipPath) { Remove-Item $zipPath -Force }
        Compress-Archive -Path (Join-Path $pkgDir '*') -DestinationPath $zipPath
        Write-Host "    압축:   $zipPath" -ForegroundColor Green
    }
}

Write-Host ""
Write-Host "완료. 배포물 위치: $DistRoot" -ForegroundColor Green
