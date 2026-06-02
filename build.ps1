#Requires -Version 5.1
<#
.SYNOPSIS
  caps-hangul-rs 릴리스 빌드 + 배포 패키지 생성.

.DESCRIPTION
  본체 exe(caps-hangul.exe)와 주입용 TSF 리더 DLL(caps_hangul_tsf.dll)을 릴리스로 빌드하고,
  DLL 을 아키텍처 접미사로 리네이밍(caps-hangul-tsf-<x64|arm64>.dll)해 본체 옆에 동봉한
  자기완결 배포 폴더를 dist\ 아래에 만든다.

  exe(windows-sys)와 DLL(windows) 은 별도 crate 이므로 `--workspace` 로 함께 빌드해야 한다.
  본체는 자신과 같은 폴더에서 자기 아키텍처에 맞는 DLL(caps-hangul-tsf-<arch>.dll)을 로드한다
  (없으면 IME 한/영 라벨이 추정값 폴백으로 동작). 따라서 exe·DLL 은 반드시 같은 아키텍처로 짝지어
  배포한다.

  배포 폴더 구성:
    dist\caps-hangul-<arch>\
      caps-hangul.exe
      caps-hangul-tsf-<arch>.dll
      install-startup.ps1
      uninstall-startup.ps1
      README.md
      LICENSE

.PARAMETER Arch
  빌드 대상. x64 | arm64 | all. 미지정 시 현재 PC 아키텍처.

.PARAMETER Zip
  지정 시 각 배포 폴더를 dist\caps-hangul-<arch>.zip 으로 압축한다.

.EXAMPLE
  .\build.ps1
  현재 아키텍처(보통 x64)만 빌드+패키지.

.EXAMPLE
  .\build.ps1 -Arch all -Zip
  x64 / arm64 모두 빌드+패키지 후 zip 압축.

.NOTES
  선행 조건:
    rustup target add x86_64-pc-windows-msvc aarch64-pc-windows-msvc
  aarch64 cross-link 에는 Visual Studio "MSVC v143 - ARM64 빌드 도구" 컴포넌트가 필요하다.
#>
[CmdletBinding()]
param(
    [ValidateSet('x64', 'arm64', 'all')]
    [string]$Arch,
    [switch]$Zip
)

$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

# 아키텍처 → (rustc target triple, DLL 접미사) 매핑
$ArchTable = @{
    'x64'   = @{ Triple = 'x86_64-pc-windows-msvc';  Suffix = 'x64' }
    'arm64' = @{ Triple = 'aarch64-pc-windows-msvc'; Suffix = 'arm64' }
}

# 대상 미지정 시 현재 PC 아키텍처를 기본값으로 사용한다.
if (-not $Arch) {
    $os = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    $Arch = if ($os -eq 'Arm64') { 'arm64' } else { 'x64' }
    Write-Host "대상 아키텍처 미지정 → 현재 PC 아키텍처 '$Arch' 사용" -ForegroundColor DarkGray
}

$Targets = if ($Arch -eq 'all') { @('x64', 'arm64') } else { @($Arch) }

$DistRoot = Join-Path $PSScriptRoot 'dist'
$null = New-Item -ItemType Directory -Path $DistRoot -Force

foreach ($a in $Targets) {
    $triple = $ArchTable[$a].Triple
    $suffix = $ArchTable[$a].Suffix

    Write-Host ""
    Write-Host "==> [$a] cargo build --release --workspace --target $triple" -ForegroundColor Cyan
    cargo build --release --workspace --target $triple
    if ($LASTEXITCODE -ne 0) {
        throw "[$a] cargo 빌드 실패 (exit $LASTEXITCODE). arm64 는 VS 'MSVC v143 - ARM64 빌드 도구' 컴포넌트와 'rustup target add $triple' 가 필요합니다."
    }

    $relDir = Join-Path $PSScriptRoot "target\$triple\release"
    $exe = Join-Path $relDir 'caps-hangul.exe'
    $dll = Join-Path $relDir 'caps_hangul_tsf.dll'
    foreach ($p in @($exe, $dll)) {
        if (-not (Test-Path $p)) { throw "산출물 누락: $p" }
    }

    # 배포 폴더는 매번 새로 만든다(스테일 파일 방지).
    $pkgName = "caps-hangul-$suffix"
    $pkgDir = Join-Path $DistRoot $pkgName
    if (Test-Path $pkgDir) { Remove-Item $pkgDir -Recurse -Force }
    $null = New-Item -ItemType Directory -Path $pkgDir -Force

    Copy-Item $exe (Join-Path $pkgDir 'caps-hangul.exe')
    Copy-Item $dll (Join-Path $pkgDir "caps-hangul-tsf-$suffix.dll")
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
