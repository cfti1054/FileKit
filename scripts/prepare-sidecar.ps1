<#
.SYNOPSIS
  convtool.exe(CLI)를 빌드해 Tauri externalBin 사이드카 자리에 둔다.

.DESCRIPTION
  1) cargo build --profile <Profile> --locked -p convtool-cli
  2) <CARGO_TARGET_DIR 또는 repo/target>/<profile 폴더>/convtool.exe 를
     app/src-tauri/binaries/convtool-<rustc -vV의 host>.exe 로 복사한다.
  tauri.release.conf.json의 "externalBin": ["binaries/convtool"]과 짝이다.
  설치하면 번들러가 트리플 접미사를 떼어 $INSTDIR\convtool.exe가 된다.

  $env:CARGO_TARGET_DIR가 상대 경로면 저장소 루트 기준으로 풀고, 자식 프로세스에도 절대 경로로 넘긴다
  (tauri build는 app/src-tauri에서 cargo를 돌리므로 상대 경로가 다른 곳을 가리키게 된다).
  성공하면 복사한 파일 경로를 출력하고 exit 0, 실패하면 exit 1.
  PowerShell 5.1과 pwsh 7에서 동작한다. 이 파일은 UTF-8 BOM으로 저장한다.

.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/prepare-sidecar.ps1
#>
[CmdletBinding()]
param(
    # $PROFILE은 자동 변수라 매개변수 이름으로 쓰지 않는다. -Profile로도 부를 수 있다.
    [Alias('Profile')]
    [string]$CargoProfile = 'release'
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot

function Find-Tool([string]$Name) {
    $cmd = Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($cmd) { return $cmd.Source }
    # rustup 설치 직후 세션은 PATH가 갱신되지 않는다.
    $fallback = Join-Path $env:USERPROFILE (".cargo\bin\{0}.exe" -f $Name)
    if (Test-Path -LiteralPath $fallback) { return $fallback }
    throw ("{0}을(를) 찾지 못함 (PATH, %USERPROFILE%\.cargo\bin)" -f $Name)
}

function Invoke-Native([string]$Exe, [string[]]$ArgList) {
    # 5.1에서 $ErrorActionPreference='Stop'이면 네이티브 stderr가 오류로 바뀌는 경우가 있어 잠시 풀어 둔다.
    $old = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $Exe @ArgList | Out-Host
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $old
    }
    if ($code -ne 0) { throw ("{0} {1} 실패 (exit {2})" -f $Exe, ($ArgList -join ' '), $code) }
}

try {
    $cargo = Find-Tool 'cargo'
    $rustc = Find-Tool 'rustc'

    if ($env:CARGO_TARGET_DIR) {
        if ([System.IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) {
            $targetDir = $env:CARGO_TARGET_DIR
        } else {
            $targetDir = [System.IO.Path]::GetFullPath((Join-Path $repo $env:CARGO_TARGET_DIR))
        }
        $env:CARGO_TARGET_DIR = $targetDir
    } else {
        $targetDir = Join-Path $repo 'target'
    }

    $hostLine = (& $rustc -vV) | Where-Object { $_ -match '^host:\s*(\S+)' } | Select-Object -First 1
    if (-not $hostLine -or -not ($hostLine -match '^host:\s*(\S+)')) { throw 'rustc -vV에서 host를 읽지 못함' }
    $triple = $Matches[1]

    # cargo의 dev 프로필 출력 폴더 이름은 debug다.
    $profileDir = $CargoProfile
    if ($CargoProfile -eq 'dev') { $profileDir = 'debug' }

    Write-Host ("[prepare-sidecar] cargo build --profile {0} --locked -p convtool-cli (target: {1})" -f $CargoProfile, $targetDir)
    Push-Location $repo
    try {
        Invoke-Native $cargo @('build', '--profile', $CargoProfile, '--locked', '-p', 'convtool-cli')
    } finally {
        Pop-Location
    }

    $built = Join-Path $targetDir (Join-Path $profileDir 'convtool.exe')
    if (-not (Test-Path -LiteralPath $built)) { throw ("빌드 결과가 없음: {0}" -f $built) }

    $binDir = Join-Path $repo 'app\src-tauri\binaries'
    if (-not (Test-Path -LiteralPath $binDir)) { New-Item -ItemType Directory -Path $binDir | Out-Null }
    $dest = Join-Path $binDir ("convtool-{0}.exe" -f $triple)
    Copy-Item -LiteralPath $built -Destination $dest -Force

    Write-Host ("[prepare-sidecar] {0} -> {1}" -f $built, $dest)
    Write-Output $dest
    exit 0
} catch {
    Write-Host ("[prepare-sidecar] 실패: {0}" -f $_.Exception.Message)
    exit 1
}
