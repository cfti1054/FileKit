<#
.SYNOPSIS
  세 파일의 버전이 같은지, 그리고 -Tag가 "v" + 버전과 같은지 확인한다.

.DESCRIPTION
  - Cargo.toml            [workspace.package] version
  - app/src-tauri/tauri.conf.json   version
  - app/ui/package.json   version
  하나라도 다르거나 읽지 못하면 내용을 출력하고 exit 1. 같으면 exit 0.
  PowerShell 5.1과 pwsh 7에서 동작한다. 이 파일은 UTF-8 BOM으로 저장한다(5.1이 한글을 읽게).

.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-version.ps1 -Tag v0.1.0
#>
[CmdletBinding()]
param(
    [string]$Tag
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot

function Read-Utf8([string]$Path) {
    # 5.1의 Get-Content 기본 인코딩은 시스템 ANSI라 한글이 깨진다. .NET으로 UTF-8을 직접 읽는다.
    return [System.IO.File]::ReadAllText($Path, [System.Text.Encoding]::UTF8)
}

function Get-CargoWorkspaceVersion([string]$Path) {
    $section = ''
    foreach ($line in (Read-Utf8 $Path) -split "`r?`n") {
        $t = $line.Trim()
        if ($t -match '^\[(.+)\]$') { $section = $Matches[1].Trim(); continue }
        if ($section -eq 'workspace.package' -and $t -match '^version\s*=\s*"([^"]+)"') {
            return $Matches[1]
        }
    }
    return $null
}

function Get-JsonVersion([string]$Path) {
    $obj = (Read-Utf8 $Path) | ConvertFrom-Json
    if ($obj.PSObject.Properties.Name -contains 'version') { return [string]$obj.version }
    return $null
}

$files = @(
    @{ Name = 'Cargo.toml [workspace.package]'; Path = Join-Path $repo 'Cargo.toml'; Kind = 'cargo' },
    @{ Name = 'app/src-tauri/tauri.conf.json'; Path = Join-Path $repo 'app\src-tauri\tauri.conf.json'; Kind = 'json' },
    @{ Name = 'app/ui/package.json'; Path = Join-Path $repo 'app\ui\package.json'; Kind = 'json' }
)

$failed = $false
$versions = @()
foreach ($f in $files) {
    $v = $null
    try {
        if ($f.Kind -eq 'cargo') { $v = Get-CargoWorkspaceVersion $f.Path } else { $v = Get-JsonVersion $f.Path }
    } catch {
        Write-Host ("[check-version] {0}: 읽기 실패: {1}" -f $f.Name, $_.Exception.Message)
        $failed = $true
        continue
    }
    if (-not $v) {
        Write-Host ("[check-version] {0}: version을 찾지 못함" -f $f.Name)
        $failed = $true
        continue
    }
    Write-Host ("[check-version] {0} = {1}" -f $f.Name, $v)
    $versions += $v
}

$unique = @($versions | Select-Object -Unique)
if ($unique.Count -gt 1) {
    Write-Host ("[check-version] 버전이 서로 다름: {0}" -f ($unique -join ', '))
    $failed = $true
}

if ($Tag -and $unique.Count -eq 1) {
    $expected = 'v' + $unique[0]
    if ($Tag -cne $expected) {
        Write-Host ("[check-version] 태그 {0} != {1}" -f $Tag, $expected)
        $failed = $true
    } else {
        Write-Host ("[check-version] 태그 {0} 일치" -f $Tag)
    }
}

if ($failed) { exit 1 }
Write-Host ("[check-version] 확인됨: {0}" -f $unique[0])
exit 0
