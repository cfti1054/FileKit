<#
.SYNOPSIS
  NSIS 설치 파일(FileKit_<ver>_x64-setup.exe)과 .sha256을 만든다.

.DESCRIPTION
  1) check-version.ps1 [-Tag]          세 파일 버전 일치, 태그 일치
  2) fetch-engines.ps1                  engines/ 채우기 (-SkipFetch면 건너뜀)
     그다음 항상: engines\ 안이 허용 목록(ffmpeg, vips, pandoc, VERSIONS.json, README.md)뿐인지,
     engines\VERSIONS.json의 version·sha256이 scripts/engines.manifest.json과 같은지 확인한다.
  3) prepare-sidecar.ps1                app/src-tauri/binaries/convtool-<triple>.exe
  4) app/ui에서 npm ci                  node_modules가 없을 때만
  5) app/src-tauri에서 ../ui/node_modules/.bin/tauri build --config tauri.release.conf.json -- --locked
  6) <target>/release/bundle/nsis/*_<ver>_x64-setup.exe 를 <OutDir>/FileKit_<ver>_x64-setup.exe로 복사하고
     같은 이름 + .sha256 ("<hex>  <파일명>") 을 쓴다. 배포 이름은 GitHub Release가 비ASCII 이름을 바꾸므로 ASCII.
  7) git diff --exit-code Cargo.lock    빌드가 잠금 파일을 바꾸지 않았는지

  실패한 단계 이름을 출력하고 exit 1 (fetch-engines 실패면 그 종료 코드를 그대로 돌려준다).
  MSVC 툴체인(x86_64-pc-windows-msvc)과 WebView2가 있는 PC 또는 CI(windows-latest)에서 5단계가 된다.
  PowerShell 5.1과 pwsh 7에서 동작한다. 이 파일은 UTF-8 BOM으로 저장한다.

.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-installer.ps1 -Tag v0.1.0
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-installer.ps1 -SkipFetch
#>
[CmdletBinding()]
param(
    [string]$Tag,
    [switch]$SkipFetch,
    [string]$OutDir
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot
if (-not $OutDir) { $OutDir = Join-Path $repo 'dist' }
if (-not [System.IO.Path]::IsPathRooted($OutDir)) {
    $OutDir = [System.IO.Path]::GetFullPath((Join-Path (Get-Location).Path $OutDir))
}

# tauri build는 app/src-tauri에서 cargo를 돌린다. 상대 CARGO_TARGET_DIR는 저장소 루트 기준 절대 경로로 바꿔 둔다.
if ($env:CARGO_TARGET_DIR) {
    if (-not [System.IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) {
        $env:CARGO_TARGET_DIR = [System.IO.Path]::GetFullPath((Join-Path $repo $env:CARGO_TARGET_DIR))
    }
    $targetDir = $env:CARGO_TARGET_DIR
} else {
    $targetDir = Join-Path $repo 'target'
}

$script:step = ''
function Step([string]$Name) {
    $script:step = $Name
    Write-Host ''
    Write-Host ("==== [build-installer] {0}" -f $Name)
}

function Fail([string]$Message, [int]$Code = 1) {
    Write-Host ("[build-installer] 실패 ({0}): {1}" -f $script:step, $Message)
    exit $Code
}

function Invoke-Native([string]$Exe, [string[]]$ArgList) {
    $old = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $Exe @ArgList | Out-Host
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $old
    }
    return $code
}

function Invoke-ChildScript([string]$Name, [hashtable]$Params) {
    $path = Join-Path $PSScriptRoot $Name
    if (-not (Test-Path -LiteralPath $path)) { Fail ("{0} 없음" -f $path) }
    $global:LASTEXITCODE = 0
    & $path @Params | Out-Host
    return $LASTEXITCODE
}

function Read-Utf8([string]$Path) {
    return [System.IO.File]::ReadAllText($Path, [System.Text.Encoding]::UTF8)
}

Write-Host ("[build-installer] repo={0} target={1} out={2}" -f $repo, $targetDir, $OutDir)

# 1) 버전
Step '1/7 check-version'
$cv = @{}
if ($Tag) { $cv['Tag'] = $Tag }
$code = Invoke-ChildScript 'check-version.ps1' $cv
if ($code -ne 0) { Fail ("check-version exit {0}" -f $code) }
$version = [string]((Read-Utf8 (Join-Path $repo 'app\src-tauri\tauri.conf.json')) | ConvertFrom-Json).version

# 2) 엔진
Step '2/7 fetch-engines'
if ($SkipFetch) {
    Write-Host '[build-installer] -SkipFetch: 엔진 내려받기를 건너뜀. 지금 engines/에 있는 것을 그대로 묶는다.'
    foreach ($req in @('ffmpeg\bin\ffmpeg.exe', 'vips\bin\vips.exe', 'pandoc\pandoc.exe', 'VERSIONS.json')) {
        $p = Join-Path $repo (Join-Path 'engines' $req)
        if (-not (Test-Path -LiteralPath $p)) { Write-Warning ("engines\{0} 없음: 설치본에 이 엔진이 빠진다" -f $req) }
    }
} else {
    $code = Invoke-ChildScript 'fetch-engines.ps1' @{}
    if ($code -ne 0) { Fail ("fetch-engines exit {0}" -f $code) $code }
}

# 2단계 뒤 검사 (-SkipFetch여도 한다).
# tauri.release.conf.json은 engines/ 전체를 resources로 묶는다. 허용 목록 밖의 항목
# (engines\libreoffice\ — spec 6장 "LibreOffice 번들 안 함", fetch가 남긴 .staging-* 등)이 있으면 설치 파일에 섞이므로 멈춘다.
$enginesDir = Join-Path $repo 'engines'
$allowed = @('ffmpeg', 'vips', 'pandoc', 'VERSIONS.json', 'README.md')
if (-not (Test-Path -LiteralPath $enginesDir)) { Fail ("engines 폴더 없음: {0}" -f $enginesDir) }
$extra = @(Get-ChildItem -LiteralPath $enginesDir -Force | Where-Object { $allowed -notcontains $_.Name })
if ($extra.Count -gt 0) {
    foreach ($e in $extra) {
        $why = '허용 목록(ffmpeg, vips, pandoc, VERSIONS.json, README.md) 밖'
        if ($e.Name -eq 'libreoffice') { $why = 'LibreOffice는 번들하지 않는다(spec 6장)' }
        elseif ($e.Name -like '.staging-*') { $why = 'fetch-engines가 남긴 임시 폴더' }
        Write-Host ("[build-installer] engines\{0}: {1}" -f $e.Name, $why)
    }
    Fail ('engines\에 번들하면 안 되는 항목이 있음: {0}. 옮기거나 지운 뒤 다시 실행한다.' -f (($extra | ForEach-Object { $_.Name }) -join ', '))
}

# engines\VERSIONS.json이 manifest와 같은 엔진(version, sha256)인지. fetch 없이 묶을 때 옛 엔진이 섞이는 것을 막는다.
function Get-Prop($Obj, [string]$Name) {
    if ($null -eq $Obj) { return $null }
    $p = $Obj.PSObject.Properties[$Name]
    if ($p) { return $p.Value }
    return $null
}
$manifestPath = Join-Path $PSScriptRoot 'engines.manifest.json'
$versionsPath = Join-Path $enginesDir 'VERSIONS.json'
if (-not (Test-Path -LiteralPath $manifestPath)) { Fail ("manifest 없음: {0}" -f $manifestPath) }
if (-not (Test-Path -LiteralPath $versionsPath)) { Fail ("engines\VERSIONS.json 없음: fetch-engines.ps1을 먼저 실행한다 ({0})" -f $versionsPath) }
try {
    $manifest = (Read-Utf8 $manifestPath) | ConvertFrom-Json
    $installed = (Read-Utf8 $versionsPath) | ConvertFrom-Json
} catch {
    Fail ("manifest 또는 VERSIONS.json 파싱 실패: {0}" -f $_.Exception.Message)
}
$mismatch = @()
foreach ($m in @(Get-Prop $manifest 'engines')) {
    $id = [string](Get-Prop $m 'id')
    $have = Get-Prop $installed $id
    if ($null -eq $have) { $mismatch += ("{0}: VERSIONS.json에 없음" -f $id); continue }
    foreach ($field in @('version', 'sha256')) {
        $want = [string](Get-Prop $m $field)
        $got = [string](Get-Prop $have $field)
        if ($want -ne $got) { $mismatch += ("{0}.{1}: manifest={2} VERSIONS.json={3}" -f $id, $field, $want, $got) }
    }
}
if ($mismatch.Count -gt 0) {
    foreach ($line in $mismatch) { Write-Host ("[build-installer] {0}" -f $line) }
    Fail 'engines\VERSIONS.json이 scripts/engines.manifest.json과 다름. fetch-engines.ps1을 다시 실행한다.'
}
Write-Host '[build-installer] engines\ 허용 목록, VERSIONS.json = manifest 확인됨'

# 3) 사이드카
Step '3/7 prepare-sidecar'
$code = Invoke-ChildScript 'prepare-sidecar.ps1' @{ CargoProfile = 'release' }
if ($code -ne 0) { Fail ("prepare-sidecar exit {0}" -f $code) }

# 4) 프런트엔드 의존성
Step '4/7 npm ci'
$uiDir = Join-Path $repo 'app\ui'
if (Test-Path -LiteralPath (Join-Path $uiDir 'node_modules')) {
    Write-Host '[build-installer] app/ui/node_modules 있음: npm ci 건너뜀'
} else {
    Push-Location $uiDir
    try { $code = Invoke-Native 'npm.cmd' @('ci') } finally { Pop-Location }
    if ($code -ne 0) { Fail ("npm ci exit {0}" -f $code) }
}

# 5) Tauri 번들
Step '5/7 tauri build'
$tauriDir = Join-Path $repo 'app\src-tauri'
$tauriCli = Join-Path $uiDir 'node_modules\.bin\tauri.cmd'
if (-not (Test-Path -LiteralPath $tauriCli)) { Fail ("Tauri CLI 없음: {0}" -f $tauriCli) }
$nsisDir = Join-Path $targetDir 'release\bundle\nsis'
$started = Get-Date
Push-Location $tauriDir
try {
    # beforeBuildCommand(npm run build, cwd ../ui)는 Tauri CLI가 돌린다. `--` 뒤는 cargo build에 넘어간다.
    $code = Invoke-Native $tauriCli @('build', '--config', 'tauri.release.conf.json', '--', '--locked')
} finally {
    Pop-Location
}
if ($code -ne 0) { Fail ("tauri build exit {0}" -f $code) }

# 6) 배포 파일 이름과 해시
Step '6/7 dist'
if (-not (Test-Path -LiteralPath $nsisDir)) { Fail ("NSIS 출력 폴더 없음: {0}" -f $nsisDir) }
$pattern = '*_{0}_x64-setup.exe' -f $version
$setup = Get-ChildItem -LiteralPath $nsisDir -File |
    Where-Object { $_.Name -like $pattern -and $_.LastWriteTime -ge $started.AddSeconds(-5) } |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $setup) { Fail ("이번 빌드의 {0} 를 {1} 에서 찾지 못함" -f $pattern, $nsisDir) }

if (-not (Test-Path -LiteralPath $OutDir)) { New-Item -ItemType Directory -Path $OutDir | Out-Null }
$distName = 'FileKit_{0}_x64-setup.exe' -f $version
$distPath = Join-Path $OutDir $distName
Copy-Item -LiteralPath $setup.FullName -Destination $distPath -Force
$hash = (Get-FileHash -LiteralPath $distPath -Algorithm SHA256).Hash.ToLowerInvariant()
$shaPath = $distPath + '.sha256'
# sha256sum 형식: "<hex>  <파일명>\n", BOM 없는 UTF-8(ASCII).
[System.IO.File]::WriteAllText($shaPath, ("{0}  {1}`n" -f $hash, $distName), (New-Object System.Text.UTF8Encoding($false)))
Write-Host ("[build-installer] {0} -> {1}" -f $setup.FullName, $distPath)
Write-Host ("[build-installer] SHA256 {0}" -f $hash)

# 7) 잠금 파일
Step '7/7 Cargo.lock'
Push-Location $repo
try { $code = Invoke-Native 'git.exe' @('diff', '--exit-code', '--', 'Cargo.lock') } finally { Pop-Location }
if ($code -ne 0) { Fail 'Cargo.lock이 바뀜 (cargo update 금지, --locked 확인)' }

Write-Host ''
Write-Host ("[build-installer] 완료: {0}" -f $distPath)
exit 0
