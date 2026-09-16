# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

function Get-LicenseFromDir {
    param([string]$Dir)
    $out = @()
    if (-not (Test-Path $Dir)) { return $out }
    foreach ($f in Get-ChildItem $Dir -File -ErrorAction SilentlyContinue) {
        if ($licenseNamesLower -contains $f.Name.ToLower()) {
            $out += [PSCustomObject]@{
                Name = $f.Name
                Text = [System.IO.File]::ReadAllText($f.FullName)
            }
        }
    }
    return $out
}

function Get-LicenseText {
    param(
        [string]$Name,
        [string]$Version,
        [string]$RepoUrl,
        [string]$ManifestPath,
        [bool]$GitSource
    )
    if ($ManifestPath) {
        $found = Get-LicenseFromDir (Split-Path -Parent $ManifestPath)
        if ($found.Count -gt 0) { return $found }
    }
    if ($GitSource) {
        if ($ManifestPath) {
            $directory = [System.IO.Path]::GetFullPath((Split-Path -Parent $ManifestPath))
            $checkout = @(& git -C $directory rev-parse --show-toplevel)
            if ($LASTEXITCODE -ne 0 -or $checkout.Count -ne 1) {
                throw "Cannot locate the resolved Git checkout for dependency '$Name'."
            }
            $root = [System.IO.Path]::GetFullPath($checkout[0])
            $ancestors = [System.Collections.Generic.List[string]]::new()
            while ($directory -ne $root) {
                $parent = Split-Path -Parent $directory
                if (-not $parent -or $parent -eq $directory) {
                    throw "Dependency '$Name' is not beneath its resolved Git checkout root."
                }
                $ancestors.Add($parent)
                $directory = $parent
            }
            # Never search outside the resolved checkout or combine a member's license with its parent's.
            foreach ($ancestor in $ancestors) {
                $found = Get-LicenseFromDir $ancestor
                if ($found.Count -gt 0) { return $found }
            }
        }
        throw "Git dependency '$Name' must include license text in its resolved checkout; refusing registry or moving-HEAD substitution."
    }
    if (Test-Path $srcRoot) {
        foreach ($reg in Get-ChildItem $srcRoot -Directory -ErrorAction SilentlyContinue) {
            $found = Get-LicenseFromDir (Join-Path $reg.FullName "$Name-$Version")
            if ($found.Count -gt 0) { return $found }
        }
    }
    $found = Get-LicenseFromCrate -Name $Name -Version $Version
    if ($found.Count -gt 0) { return $found }
    $upstream = Get-LicenseFromGithub -RepoUrl $RepoUrl
    if ($upstream) { return @($upstream) }
    return @()
}

function Get-WtaDependencyProvenance {
    [CmdletBinding()]
    param([Parameter(Mandatory)]$Package)

    if ([string]$Package.source -like 'git+*') {
        $source = ([string]$Package.source).Substring(4)
        $separator = $source.LastIndexOf('#')
        if ($separator -le 0 -or $source.Substring($separator + 1) -notmatch '^(?:[0-9a-fA-F]{40}|[0-9a-fA-F]{64})$') {
            throw 'Git dependency provenance requires a full resolved commit.'
        }
        $commit = $source.Substring($separator + 1).ToLowerInvariant()
        $repository = ($source.Substring(0, $separator) -split '\?', 2)[0]
        $uri = $null
        if (-not [uri]::TryCreate($repository, [UriKind]::Absolute, [ref]$uri) -or
            $uri.Scheme -notin @('https', 'http', 'ssh', 'git') -or -not $uri.Host) {
            throw 'Git dependency provenance requires a public repository URL.'
        }
        if ($uri.UserInfo -and -not ($uri.Scheme -eq 'ssh' -and $uri.UserInfo -eq 'git')) {
            throw 'Git dependency provenance must not contain credentials.'
        }
        $repository = $repository.TrimEnd('/') -replace '\.git$', ''
        $url = if ($uri.Host -eq 'github.com') {
            $path = $uri.AbsolutePath.TrimEnd('/') -replace '\.git$', ''
            "https://github.com$path/tree/$commit"
        }
        else { $repository }

        return [pscustomobject]@{
            Component = [pscustomobject]@{
                type = 'git'
                git = [pscustomobject]@{
                    repositoryUrl = $repository
                    commitHash = $commit
                }
            }
            SourceUrl = $url
            IsGit = $true
            LicenseDirectory = Split-Path -Parent $Package.manifest_path
        }
    }

    $url = if ($Package.repository) { $Package.repository }
        elseif ($Package.homepage) { $Package.homepage }
        else { "https://crates.io/crates/$($Package.name)" }

    [pscustomobject]@{
        Component = [pscustomobject]@{
            type = 'cargo'
            cargo = [pscustomobject]@{
                name = $Package.name
                version = $Package.version
            }
        }
        SourceUrl = $url
        IsGit = $false
        LicenseDirectory = Split-Path -Parent $Package.manifest_path
    }
}
