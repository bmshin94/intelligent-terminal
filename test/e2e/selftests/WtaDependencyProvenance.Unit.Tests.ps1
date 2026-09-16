# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\..\..\build\scripts\WtaDependencyProvenance.ps1')

    function New-TestDependency {
        param([string]$Source)
        [pscustomobject]@{
            name = 'example-markdown'
            version = '0.3.9'
            source = $Source
            repository = 'https://github.com/upstream/example-markdown'
            homepage = $null
            manifest_path = 'C:\cargo\git\checkouts\example\crate\Cargo.toml'
        }
    }
    $script:commit = '0123456789abcdef0123456789abcdef01234567'
}

Describe 'WTA dependency provenance' -Tag Unit {
    It 'keeps registry component identity unchanged' {
        $package = New-TestDependency 'registry+https://github.com/rust-lang/crates.io-index'
        $result = Get-WtaDependencyProvenance -Package $package
        $result.Component.type | Should -Be 'cargo'
        $result.Component.cargo.name | Should -Be 'example-markdown'
        $result.Component.cargo.version | Should -Be '0.3.9'
        $result.SourceUrl | Should -Be $package.repository
        $result.IsGit | Should -BeFalse
    }

    Describe 'WTA dependency license lookup' -Tag Unit {
        BeforeAll {
            $script:licenseNamesLower = @('license-mit', 'license-apache')
            function Get-LicenseFromCrate { throw 'Registry substitution must not occur.' }
            function Get-LicenseFromGithub { throw 'Moving-HEAD substitution must not occur.' }
        }

        BeforeEach {
            $checkout = Join-Path $TestDrive ([guid]::NewGuid().ToString('N'))
            $crate = Join-Path $checkout 'members\example'
            New-Item -ItemType Directory -Path $crate -Force | Out-Null
            git init --quiet $checkout
            if ($LASTEXITCODE -ne 0) { throw 'Cannot initialize test checkout.' }
            $manifest = Join-Path $crate 'Cargo.toml'
            Set-Content -LiteralPath $manifest -Value '[package]'
        }

        It 'finds both licenses at the root of a Git workspace' {
            Set-Content -LiteralPath (Join-Path $checkout 'LICENSE-MIT') -Value 'workspace MIT text'
            Set-Content -LiteralPath (Join-Path $checkout 'LICENSE-APACHE') -Value 'workspace Apache text'
            $result = @(Get-LicenseText -Name example -Version 1.0 -ManifestPath $manifest -GitSource $true)
            $result.Count | Should -Be 2
            ($result | Where-Object Name -eq LICENSE-MIT).Text.Trim() | Should -Be 'workspace MIT text'
            ($result | Where-Object Name -eq LICENSE-APACHE).Text.Trim() | Should -Be 'workspace Apache text'
        }

        It 'prefers crate-local licenses over workspace licenses' {
            Set-Content -LiteralPath (Join-Path $checkout 'LICENSE-MIT') -Value 'workspace text'
            Set-Content -LiteralPath (Join-Path $crate 'LICENSE-MIT') -Value 'crate text'
            $result = @(Get-LicenseText -Name example -Version 1.0 -ManifestPath $manifest -GitSource $true)
            $result.Count | Should -Be 1
            $result[0].Text.Trim() | Should -Be 'crate text'
        }

        It 'uses the nearest licensed ancestor inside the Git checkout' {
            Set-Content -LiteralPath (Join-Path $checkout 'LICENSE-MIT') -Value 'workspace text'
            Set-Content -LiteralPath (Join-Path (Split-Path -Parent $crate) 'LICENSE-MIT') -Value 'member text'
            $result = @(Get-LicenseText -Name example -Version 1.0 -ManifestPath $manifest -GitSource $true)
            $result[0].Text.Trim() | Should -Be 'member text'
        }

        It 'does not substitute licenses outside the Git checkout or from the registry' {
            Set-Content -LiteralPath (Join-Path $TestDrive 'LICENSE-MIT') -Value 'unrelated text'
            { Get-LicenseText -Name example -Version 1.0 -ManifestPath $manifest -GitSource $true } |
                Should -Throw '*refusing registry or moving-HEAD substitution*'
        }

        It 'keeps registry package-local license lookup unchanged' {
            Set-Content -LiteralPath (Join-Path $crate 'LICENSE-MIT') -Value 'registry text'
            $result = @(Get-LicenseText -Name example -Version 1.0 -ManifestPath $manifest -GitSource $false)
            $result[0].Text.Trim() | Should -Be 'registry text'
        }
    }

    It 'attributes a pinned fork to its actual repository and commit' {
        $package = New-TestDependency "git+https://github.com/contributor/example-markdown?rev=$script:commit#$script:commit"
        $result = Get-WtaDependencyProvenance -Package $package
        $result.Component.type | Should -Be 'git'
        $result.Component.git.repositoryUrl | Should -Be 'https://github.com/contributor/example-markdown'
        $result.Component.git.commitHash | Should -Be $script:commit
        $result.SourceUrl | Should -Be "https://github.com/contributor/example-markdown/tree/$script:commit"
        $result.IsGit | Should -BeTrue
        $result.LicenseDirectory | Should -Be 'C:\cargo\git\checkouts\example\crate'
    }

    It 'rejects git provenance without a resolved immutable commit' {
        $package = New-TestDependency 'git+https://github.com/contributor/example-markdown?branch=main'
        { Get-WtaDependencyProvenance -Package $package } | Should -Throw '*resolved commit*'
    }

    It 'does not emit credentials embedded in an HTTP source URL' {
        $package = New-TestDependency "git+https://private-token@github.com/contributor/example-markdown#$script:commit"
        { Get-WtaDependencyProvenance -Package $package } | Should -Throw '*credentials*'
    }

    It 'rejects machine-local git sources in distributable provenance' {
        $package = New-TestDependency "git+file:///C:/private/example-markdown#$script:commit"
        { Get-WtaDependencyProvenance -Package $package } | Should -Throw '*public repository URL*'
    }
}
