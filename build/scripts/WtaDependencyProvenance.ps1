# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

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
