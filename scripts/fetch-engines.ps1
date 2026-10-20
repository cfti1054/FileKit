<#
.SYNOPSIS
    변환 엔진(FFmpeg, libvips, Pandoc)을 고정 버전 zip으로 내려받아 engines/<id>/에 배치한다.

.DESCRIPTION
    scripts/engines.manifest.json의 엔진마다:
      1. 캐시 폴더에 zip을 받는다. 캐시에 있고 SHA256이 맞으면 다시 받지 않는다.
      2. SHA256을 manifest 값과 비교한다(다르면 종료 코드 2).
      3. 시스템 임시 폴더에 푼다. 항목 경로를 검사해 임시 폴더 밖으로 나가는 항목(zip slip)은 거부한다.
         맨 위 폴더를 떼고(strip_root) keep glob에 맞는 파일만 남긴다.
      4. required 파일이 모두 있는지 확인한 뒤(없으면 종료 코드 3) engines/<id>/를 지우고 새로 만든다.
    끝나면 engines/VERSIONS.json을 쓴다. engines/README.md와 engines/libreoffice/는 건드리지 않는다.

    내려받은 파일은 신뢰하지 않는 데이터로 다룬다. 이 스크립트는 압축을 풀기만 하고 안의 실행 파일을 실행하지 않는다.
    배치 확인은 `convtool engines --json`으로 한다.

    PowerShell 5.1과 PowerShell 7 둘 다에서 동작한다. 이 파일은 UTF-8 BOM으로 저장해야 5.1이 한글을 읽는다.

    종료 코드: 0 성공, 1 내려받기·압축 풀기·manifest 오류, 2 SHA256 불일치, 3 배치 검사 실패

.PARAMETER Manifest
    manifest 경로. 기본 scripts/engines.manifest.json
.PARAMETER Dest
    배치할 engines 폴더. 기본 <저장소>/engines
.PARAMETER Only
    이 엔진만 처리한다. 예: -Only ffmpeg,vips
.PARAMETER Cache
    zip 캐시 폴더. 기본 $env:CONVTOOL_ENGINE_CACHE, 없으면 <저장소>/.cache/engines
.PARAMETER Force
    캐시를 무시하고 다시 내려받는다.
.PARAMETER PrintHashes
    검증·배치 없이 zip의 SHA256만 출력한다(manifest 갱신용, 처음 받을 때 믿고 기록하는 방식).

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts/fetch-engines.ps1
.EXAMPLE
    pwsh -File scripts/fetch-engines.ps1 -Only pandoc -Force
#>
[CmdletBinding()]
param(
    [string]$Manifest,
    [string]$Dest,
    [string[]]$Only,
    [string]$Cache,
    [switch]$Force,
    [switch]$PrintHashes
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# 압축 해제 크기 상한(zip bomb 방지). 지금 엔진 중 가장 큰 FFmpeg가 압축 해제 후 약 500MB다.
$script:MaxExtractBytes = 4GB

# ---------------------------------------------------------------------------
# 순수 함수 (파일·네트워크를 건드리지 않는다. 점 소싱으로 따로 시험할 수 있다)
# ---------------------------------------------------------------------------

function New-FetchError {
    param([int]$Code, [string]$Message)
    $e = New-Object System.Exception $Message
    $e.Data['FetchExitCode'] = $Code
    return $e
}

function ConvertTo-GlobRegex {
    # 상대 경로 glob → 정규식. '/' 구분. '**'는 '/'를 포함한 아무 문자열, '*'와 '?'는 한 단계 안에서만.
    # 대소문자 구분 없음(Windows 파일 시스템).
    param([Parameter(Mandatory)][string]$Glob)
    $g = $Glob.Replace('\', '/')
    $sb = New-Object System.Text.StringBuilder
    [void]$sb.Append('^')
    $i = 0
    while ($i -lt $g.Length) {
        $c = $g[$i]
        if ($c -eq '*') {
            if ($i + 1 -lt $g.Length -and $g[$i + 1] -eq '*') {
                if ($i + 2 -lt $g.Length -and $g[$i + 2] -eq '/') {
                    [void]$sb.Append('(?:.*/)?')   # '**/' = 0단계 이상
                    $i += 3
                } else {
                    [void]$sb.Append('.*')
                    $i += 2
                }
                continue
            }
            [void]$sb.Append('[^/]*')
        } elseif ($c -eq '?') {
            [void]$sb.Append('[^/]')
        } else {
            [void]$sb.Append([regex]::Escape([string]$c))
        }
        $i++
    }
    [void]$sb.Append('$')
    return $sb.ToString()
}

function Test-KeepPath {
    # $Keep가 비었으면 전부 남긴다.
    param([Parameter(Mandatory)][string]$RelPath, [string[]]$Keep)
    if ($null -eq $Keep -or $Keep.Count -eq 0) { return $true }
    foreach ($k in $Keep) {
        if ($RelPath -match ('(?i)' + (ConvertTo-GlobRegex $k))) { return $true }
    }
    return $false
}

function Get-SafeEntryPath {
    # zip 항목 이름을 검사해 '/'로 정규화한 상대 경로를 돌려준다. 위험하면 $null.
    # 거부: 빈 이름, 절대 경로, 드라이브 문자, UNC, '..' 단계, ':'(대체 데이터 스트림), 제어 문자,
    #       Windows 예약 이름, 끝이 '.'이나 ' '인 단계.
    param([Parameter(Mandatory)][AllowEmptyString()][string]$EntryName)
    $n = $EntryName.Replace('\', '/')
    if ($n.Length -eq 0) { return $null }
    if ($n.StartsWith('/')) { return $null }
    if ($n -match '[:\x00-\x1f<>"|?*]') { return $null }
    $parts = @($n.TrimEnd('/').Split('/') | Where-Object { $_ -ne '' -and $_ -ne '.' })
    if ($parts.Count -eq 0) { return $null }
    foreach ($p in $parts) {
        if ($p -eq '..') { return $null }
        if ($p.EndsWith('.') -or $p.EndsWith(' ')) { return $null }
        if ($p -match '^(?i)(CON|PRN|AUX|NUL|COM[0-9]|LPT[0-9])(\..*)?$') { return $null }
    }
    return ($parts -join '/')
}

function Get-StrippedPath {
    # 맨 위 폴더 $Root를 뗀다. $Root 밖의 항목이거나 $Root 자체면 $null.
    param([Parameter(Mandatory)][string]$RelPath, [string]$Root)
    if (-not $Root) { return $RelPath }
    $prefix = $Root + '/'
    if ($RelPath.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        $rest = $RelPath.Substring($prefix.Length)
        if ($rest.Length -gt 0) { return $rest }
    }
    return $null
}

function Get-CommonRoot {
    # 모든 항목이 같은 맨 위 폴더 아래에 있으면 그 이름, 아니면 $null.
    param([string[]]$RelPaths)
    $root = $null
    foreach ($p in $RelPaths) {
        $i = $p.IndexOf('/')
        if ($i -lt 1) { return $null }   # 맨 위에 파일이 있음
        $first = $p.Substring(0, $i)
        if ($null -eq $root) { $root = $first }
        elseif ($root -ne $first) { return $null }
    }
    return $root
}

function Test-EngineUrl {
    # 문제가 없으면 $null, 있으면 이유 문자열.
    param([Parameter(Mandatory)][string]$Id, [Parameter(Mandatory)][string]$Url)
    if ($Url -notmatch '^https://') { return 'https URL이 아닙니다' }
    if ($Url -match '(?i)latest') { return "고정 버전 URL이 아닙니다('latest' 포함)" }
    if ($Id -eq 'ffmpeg') {
        $file = $Url.Substring($Url.LastIndexOf('/') + 1)
        if ($file -match '(?i)-gpl') { return 'FFmpeg GPL 빌드는 쓰지 않습니다(LGPL 빌드만)' }
        if ($file -notmatch '(?i)win64-lgpl') { return "FFmpeg는 'win64-lgpl' 빌드만 씁니다" }
        if ($file -match '(?i)shared') { return "FFmpeg '-shared' 빌드는 쓰지 않습니다" }
    }
    return $null
}

function Test-Sha256Text {
    param([AllowNull()][AllowEmptyString()][string]$Text)
    return ($null -ne $Text -and $Text -match '^[0-9a-fA-F]{64}$')
}

function Get-ManifestValue {
    param($Obj, [string]$Name)
    $prop = $Obj.PSObject.Properties[$Name]
    if ($null -eq $prop) { return $null }
    return $prop.Value
}

# ---------------------------------------------------------------------------
# 파일·네트워크 작업
# ---------------------------------------------------------------------------

function Get-FileSha256 {
    param([Parameter(Mandatory)][string]$Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Invoke-Download {
    param([Parameter(Mandatory)][string]$Url, [Parameter(Mandatory)][string]$OutFile)
    $part = $OutFile + '.part'
    $last = $null
    for ($try = 1; $try -le 3; $try++) {
        try {
            if (Test-Path -LiteralPath $part) { Remove-Item -LiteralPath $part -Force }
            Invoke-WebRequest -Uri $Url -OutFile $part -UseBasicParsing -MaximumRedirection 10 -TimeoutSec 600
            Move-Item -LiteralPath $part -Destination $OutFile -Force
            return
        } catch {
            $last = $_.Exception.Message
            Write-Host ("  내려받기 실패({0}/3): {1}" -f $try, $last) -ForegroundColor Yellow
            Start-Sleep -Seconds (2 * $try)
        }
    }
    if (Test-Path -LiteralPath $part) { Remove-Item -LiteralPath $part -Force -ErrorAction SilentlyContinue }
    throw (New-FetchError 1 "내려받기 실패: $Url`n  $last")
}

function Get-EngineZip {
    # 캐시 경로의 zip을 준비한다. $ExpectedSha가 있으면 캐시 재사용 여부를 그것으로 판단한다.
    param($Engine, [string]$CacheDir, [string]$ExpectedSha, [switch]$NoCache)
    $file = $Engine.url.Substring($Engine.url.LastIndexOf('/') + 1)
    if ($file -notmatch '^[A-Za-z0-9._+-]+\.zip$') {
        throw (New-FetchError 1 "$($Engine.id): URL의 파일 이름이 zip이 아니거나 이상합니다: $file")
    }
    $path = Join-Path $CacheDir ($Engine.id + '-' + $file)
    if (-not $NoCache -and (Test-Path -LiteralPath $path -PathType Leaf)) {
        if (-not $ExpectedSha) {
            Write-Host "  캐시 사용: $path"
            return $path
        }
        $sha = Get-FileSha256 $path
        if ($sha -eq $ExpectedSha.ToLowerInvariant()) {
            Write-Host "  캐시 사용(SHA256 일치): $path"
            return $path
        }
        Write-Host "  캐시 SHA256이 다름. 다시 받습니다: $path" -ForegroundColor Yellow
        Remove-Item -LiteralPath $path -Force
    }
    Write-Host "  내려받기: $($Engine.url)"
    Invoke-Download -Url $Engine.url -OutFile $path
    return $path
}

function Expand-EngineZip {
    # zip을 $WorkDir에 푼다. strip_root·keep을 적용한 상대 경로 목록을 돌려준다.
    param([Parameter(Mandatory)][string]$ZipPath, [Parameter(Mandatory)][string]$WorkDir, [bool]$StripRoot, [string[]]$Keep, [string]$Id)
    $zip = $null
    try {
        try {
            $zip = [System.IO.Compression.ZipFile]::OpenRead($ZipPath)
        } catch {
            throw (New-FetchError 1 "${Id}: zip을 열 수 없습니다: $($_.Exception.Message)")
        }
        $files = New-Object System.Collections.ArrayList
        foreach ($entry in $zip.Entries) {
            $isDir = $entry.FullName.EndsWith('/') -or $entry.FullName.EndsWith('\')
            $rel = Get-SafeEntryPath $entry.FullName
            if ($null -eq $rel) {
                throw (New-FetchError 1 "${Id}: 위험한 zip 항목 이름을 거부합니다: '$($entry.FullName)'")
            }
            if (-not $isDir) { [void]$files.Add([pscustomobject]@{ Rel = $rel; Entry = $entry }) }
        }
        $root = $null
        if ($StripRoot) {
            $root = Get-CommonRoot ([string[]]@($files | ForEach-Object { $_.Rel }))
            if (-not $root) {
                throw (New-FetchError 3 "${Id}: strip_root=true인데 zip에 맨 위 폴더가 하나가 아닙니다")
            }
            Write-Host "  맨 위 폴더 뗌: $root/"
        }
        $baseFull = [System.IO.Path]::GetFullPath($WorkDir).TrimEnd('\') + '\'
        $total = [int64]0
        $kept = New-Object System.Collections.ArrayList
        foreach ($f in $files) {
            $rel = Get-StrippedPath $f.Rel $root
            if ($null -eq $rel) { continue }
            if (-not (Test-KeepPath $rel $Keep)) { continue }
            $total += $f.Entry.Length
            if ($total -gt $script:MaxExtractBytes) {
                throw (New-FetchError 1 "${Id}: 압축 해제 크기가 상한($($script:MaxExtractBytes) 바이트)을 넘습니다")
            }
            $target = [System.IO.Path]::GetFullPath((Join-Path $WorkDir ($rel.Replace('/', '\'))))
            if (-not $target.StartsWith($baseFull, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw (New-FetchError 1 "${Id}: 임시 폴더 밖을 가리키는 항목을 거부합니다: '$($f.Entry.FullName)'")
            }
            $parent = [System.IO.Path]::GetDirectoryName($target)
            if (-not (Test-Path -LiteralPath $parent)) { [void](New-Item -ItemType Directory -Path $parent -Force) }
            try {
                [System.IO.Compression.ZipFileExtensions]::ExtractToFile($f.Entry, $target, $false)
            } catch {
                throw (New-FetchError 1 "${Id}: 압축 풀기 실패 '$rel': $($_.Exception.Message)")
            }
            [void]$kept.Add($rel)
        }
        return , ([string[]]$kept.ToArray())
    } finally {
        if ($null -ne $zip) { $zip.Dispose() }
    }
}

function Test-PeFile {
    param([Parameter(Mandatory)][string]$Path)
    $fs = [System.IO.File]::OpenRead($Path)
    try {
        $b = New-Object byte[] 2
        $n = $fs.Read($b, 0, 2)
        return ($n -eq 2 -and $b[0] -eq 0x4D -and $b[1] -eq 0x5A)   # 'MZ'
    } finally { $fs.Dispose() }
}

function Read-VersionsFile {
    param([string]$Path)
    $map = [ordered]@{}
    if (Test-Path -LiteralPath $Path -PathType Leaf) {
        try {
            $obj = Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json
            foreach ($p in $obj.PSObject.Properties) { $map[$p.Name] = $p.Value }
        } catch {
            Write-Host "  경고: 기존 VERSIONS.json을 읽지 못해 새로 씁니다: $($_.Exception.Message)" -ForegroundColor Yellow
        }
    }
    return $map
}

function Remove-Tree {
    # 막 쓴 실행 파일은 백신 검사가 잠시 잡고 있어 한 번에 지워지지 않을 수 있다. 몇 번 다시 시도한다.
    param([Parameter(Mandatory)][string]$Path, [switch]$Quiet)
    for ($try = 1; $try -le 5; $try++) {
        if (-not (Test-Path -LiteralPath $Path)) { return $true }
        try {
            Remove-Item -LiteralPath $Path -Recurse -Force
        } catch {
            if ($try -eq 5) {
                if (-not $Quiet) { throw }
                Write-Host "  경고: 지우지 못했습니다(직접 지우세요): $Path" -ForegroundColor Yellow
                return $false
            }
            Start-Sleep -Milliseconds (500 * $try)
        }
    }
    return (-not (Test-Path -LiteralPath $Path))
}

function Write-Utf8File {
    param([string]$Path, [string]$Text)
    $enc = New-Object System.Text.UTF8Encoding $false
    [System.IO.File]::WriteAllText($Path, $Text, $enc)
}

# ---------------------------------------------------------------------------
# 본문
# ---------------------------------------------------------------------------

function Invoke-FetchEngines {
    $repo = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
    $manifestPath = if ($Manifest) { $Manifest } else { Join-Path $PSScriptRoot 'engines.manifest.json' }
    $destDir = if ($Dest) { $Dest } else { Join-Path $repo 'engines' }
    $cacheDir = if ($Cache) { $Cache } elseif ($env:CONVTOOL_ENGINE_CACHE) { $env:CONVTOOL_ENGINE_CACHE } else { Join-Path $repo '.cache\engines' }
    $destDir = [System.IO.Path]::GetFullPath($destDir)
    $cacheDir = [System.IO.Path]::GetFullPath($cacheDir)

    # TLS 1.2 강제 (PowerShell 5.1 기본값은 TLS 1.0일 수 있다)
    [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.ServicePointManager]::SecurityProtocol -bor [System.Net.SecurityProtocolType]::Tls12
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem

    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
        throw (New-FetchError 1 "manifest가 없습니다: $manifestPath")
    }
    try {
        $m = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
    } catch {
        throw (New-FetchError 1 "manifest JSON을 읽지 못했습니다: $($_.Exception.Message)")
    }
    $engines = @(Get-ManifestValue $m 'engines')
    if ($engines.Count -eq 0 -or $null -eq $engines[0]) { throw (New-FetchError 1 'manifest에 engines가 없습니다') }

    # manifest 검사
    $ids = @()
    foreach ($e in $engines) {
        $id = Get-ManifestValue $e 'id'
        if (-not $id -or $id -notmatch '^[a-z0-9_-]+$') { throw (New-FetchError 1 "manifest: id가 잘못됐습니다: '$id'") }
        if ($id -eq 'libreoffice') { throw (New-FetchError 1 'manifest: libreoffice는 번들하지 않습니다') }
        if ($ids -contains $id) { throw (New-FetchError 1 "manifest: id가 겹칩니다: $id") }
        $ids += $id
        $url = Get-ManifestValue $e 'url'
        if (-not $url) { throw (New-FetchError 1 "manifest: ${id}에 url이 없습니다") }
        $why = Test-EngineUrl $id $url
        if ($why) { throw (New-FetchError 1 "manifest: ${id}: $why`n  $url") }
        if (-not (Get-ManifestValue $e 'version')) { throw (New-FetchError 1 "manifest: ${id}에 version이 없습니다") }
        if (@(Get-ManifestValue $e 'required').Count -eq 0) { throw (New-FetchError 1 "manifest: ${id}에 required가 없습니다") }
        if (-not $PrintHashes -and -not (Test-Sha256Text (Get-ManifestValue $e 'sha256'))) {
            throw (New-FetchError 1 "manifest: ${id}의 sha256이 64자리 16진수가 아닙니다. -PrintHashes로 값을 구해 적으세요")
        }
    }
    if ($Only) {
        $want = @($Only | ForEach-Object { $_.Split(',') } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
        foreach ($w in $want) {
            if ($ids -notcontains $w) { throw (New-FetchError 1 "-Only: manifest에 없는 엔진입니다: $w (있는 것: $($ids -join ', '))") }
        }
        $engines = @($engines | Where-Object { $want -contains $_.id })
    }

    if (-not (Test-Path -LiteralPath $cacheDir)) { [void](New-Item -ItemType Directory -Path $cacheDir -Force) }

    if ($PrintHashes) {
        foreach ($e in $engines) {
            Write-Host "[$($e.id)] $($e.version)"
            $zip = Get-EngineZip -Engine $e -CacheDir $cacheDir -ExpectedSha $null -NoCache:$Force
            $sha = Get-FileSha256 $zip
            Write-Output ("{0}`t{1}`t{2}" -f $e.id, $sha, $e.url)
        }
        return
    }

    if (-not (Test-Path -LiteralPath $destDir)) { [void](New-Item -ItemType Directory -Path $destDir -Force) }
    $destFull = $destDir.TrimEnd('\') + '\'
    $versionsPath = Join-Path $destDir 'VERSIONS.json'
    $versions = Read-VersionsFile $versionsPath

    $workRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('filekit-engines-' + [guid]::NewGuid().ToString('N'))
    [void](New-Item -ItemType Directory -Path $workRoot -Force)
    try {
        foreach ($e in $engines) {
            $id = $e.id
            Write-Host "[$id] $($e.version)"
            $expected = ([string]$e.sha256).ToLowerInvariant()
            $zip = Get-EngineZip -Engine $e -CacheDir $cacheDir -ExpectedSha $expected -NoCache:$Force
            $actual = Get-FileSha256 $zip
            if ($actual -ne $expected) {
                $bad = $zip + '.mismatch'
                Move-Item -LiteralPath $zip -Destination $bad -Force
                throw (New-FetchError 2 "${id}: SHA256 불일치`n  기대: $expected`n  실제: $actual`n  파일: $bad")
            }
            Write-Host "  SHA256 확인: $actual"

            $work = Join-Path $workRoot $id
            [void](New-Item -ItemType Directory -Path $work -Force)
            $strip = [bool](Get-ManifestValue $e 'strip_root')
            $keep = [string[]]@(Get-ManifestValue $e 'keep' | Where-Object { $_ })
            $kept = Expand-EngineZip -ZipPath $zip -WorkDir $work -StripRoot $strip -Keep $keep -Id $id
            Write-Host "  남긴 파일: $($kept.Count)개"

            # 배치 검사 (임시 폴더에서, 실행하지 않고)
            foreach ($r in @(Get-ManifestValue $e 'required')) {
                $p = Join-Path $work ($r.Replace('/', '\'))
                if (-not (Test-Path -LiteralPath $p -PathType Leaf)) {
                    throw (New-FetchError 3 "${id}: 필요한 파일이 없습니다: $r")
                }
                if ($p.EndsWith('.exe', [System.StringComparison]::OrdinalIgnoreCase) -and -not (Test-PeFile $p)) {
                    throw (New-FetchError 3 "${id}: 실행 파일 형식이 아닙니다: $r")
                }
            }
            $lic = @($kept | Where-Object { ($_.Split('/')[-1]) -match '^(?i)(LICENSE|COPYING|COPYRIGHT|NOTICE)' })
            if ($lic.Count -eq 0) {
                Write-Host "  경고: ${id}에 라이선스 파일이 없습니다(keep 확인)" -ForegroundColor Yellow
            } else {
                Write-Host "  라이선스 파일: $($lic -join ', ')"
            }

            # engines/<id>/만 바꾼다. engines 안의 준비 폴더로 옮긴 뒤(볼륨이 다르면 복사) 이름을 바꾼다.
            $target = [System.IO.Path]::GetFullPath((Join-Path $destDir $id))
            if (-not $target.StartsWith($destFull, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw (New-FetchError 3 "${id}: 대상 폴더가 engines 밖입니다: $target")
            }
            $staging = Join-Path $destDir (".staging-$id")
            [void](Remove-Tree $staging)
            $moved = $false
            if ([System.IO.Path]::GetPathRoot($work) -eq [System.IO.Path]::GetPathRoot($staging)) {
                try { Move-Item -LiteralPath $work -Destination $staging; $moved = $true } catch { $moved = $false }
            }
            if (-not $moved) {
                [void](Remove-Tree $staging)
                [void](New-Item -ItemType Directory -Path $staging -Force)
                Get-ChildItem -LiteralPath $work -Force | Copy-Item -Destination $staging -Recurse -Force
            }
            if (Test-Path -LiteralPath $target) {
                try {
                    [void](Remove-Tree $target)
                } catch {
                    [void](Remove-Tree $staging -Quiet)
                    throw (New-FetchError 3 "${id}: 기존 폴더를 지우지 못했습니다(엔진이 실행 중인지 확인): $($_.Exception.Message)")
                }
            }
            Rename-Item -LiteralPath $staging -NewName $id
            Write-Host "  배치함: $target"

            $versions[$id] = [ordered]@{
                version    = [string]$e.version
                url        = [string]$e.url
                sha256     = $actual
                license    = [string](Get-ManifestValue $e 'license')
                source_url = [string](Get-ManifestValue $e 'source_url')
            }
        }
    } finally {
        [void](Remove-Tree $workRoot -Quiet)
        # 실패했어도 그때까지 배치한 엔진은 기록한다. 폴더가 없어진 항목은 뺀다.
        $out = [ordered]@{}
        foreach ($k in @($versions.Keys)) {
            if (Test-Path -LiteralPath (Join-Path $destDir $k)) { $out[$k] = $versions[$k] }
        }
        if ($out.Count -gt 0) {
            Write-Utf8File $versionsPath (($out | ConvertTo-Json -Depth 4) + "`n")
        }
    }
    Write-Host "VERSIONS.json: $versionsPath"
}

# 점 소싱(. ./fetch-engines.ps1)이면 함수만 정의하고 끝낸다(순수 함수 시험용).
if ($MyInvocation.InvocationName -eq '.') { return }

try {
    Invoke-FetchEngines
    exit 0
} catch {
    $code = 1
    if ($_.Exception.Data.Contains('FetchExitCode')) { $code = [int]$_.Exception.Data['FetchExitCode'] }
    [Console]::Error.WriteLine("fetch-engines: 오류: " + $_.Exception.Message)
    if ($code -eq 1 -and -not $_.Exception.Data.Contains('FetchExitCode')) {
        [Console]::Error.WriteLine($_.ScriptStackTrace)
    }
    exit $code
}
