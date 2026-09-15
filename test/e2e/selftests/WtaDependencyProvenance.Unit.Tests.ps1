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
