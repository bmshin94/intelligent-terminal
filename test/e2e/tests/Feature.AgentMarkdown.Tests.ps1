#Requires -Modules @{ ModuleName='Pester'; ModuleVersion='5.0.0' }
# Deterministic settings -> C++ -> helper/master -> stdio ACP -> rendered-pane
# coverage. No model, network response, or elapsed-time streaming oracle.
# Run only after selecting and verifying the deployed package revision.

BeforeDiscovery {
    $script:Ready = [bool](
        (Get-AppxPackage | Where-Object { $_.Name -like '*IntelligentTerminal*' }) -and
        (Get-Command pwsh -ErrorAction SilentlyContinue) -and
        (Get-Command winapp -ErrorAction SilentlyContinue)
    )
}

Describe 'Feature: agent Markdown rendering' -Tag 'Feature', 'AgentMarkdown' -Skip:(-not $script:Ready) {
    BeforeAll {
        Import-Module (Join-Path $PSScriptRoot '..\ItE2E\ItE2E.psd1') -Force
        $script:fixture = (Resolve-Path (Join-Path $PSScriptRoot '..\fixtures\Mock-AcpInteractionAgent.ps1')).Path
        $script:unicode = "caf`u{e9} `u{754c} e`u{301} `u{1f469}`u{200d}`u{1f4bb}"

        function Start-MarkdownTerminal {
            param([switch]$Raw)

            # Clear-WtConfig intentionally preserves unrelated settings, including
            # new keys it does not yet know. Remove this key before cold launch to
            # exercise ABSENCE, not null or an already hot-reloaded default.
            $script:backupApp = Resolve-ItApp -Package (Get-ItTestPackage)
            Stop-StaleItInstances -App $script:backupApp
            Backup-WtConfig -App $script:backupApp
            Clear-WtConfig -App $script:backupApp
            $settings = Get-WtSettingsObject -App $script:backupApp
            if ($settings -and $settings.PSObject.Properties.Name -contains 'renderAgentMarkdown') {
                $settings.PSObject.Properties.Remove('renderAgentMarkdown')
                $settings | ConvertTo-Json -Depth 64 |
                    Set-Content -LiteralPath $script:backupApp.SettingsPath -Encoding utf8
            }
            $fixtureInvocation = "& '$($script:fixture.Replace("'", "''"))' -LogPath '$($script:requestLog.Replace("'", "''"))'"
            $encodedInvocation = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($fixtureInvocation))
            $launchSettings = @{
                acpAgent = 'custom:markdown-fixture'
                acpCustomCommand = "pwsh -NoProfile -EncodedCommand $encodedInvocation"
                acpModel = ''
                agentPanePosition = 'right'
                autoErrorDetectionEnabled = $true
                autoFixEnabled = $false
                initialCols = 140
                initialRows = 50
                launchMode = 'maximized'
            }
            if ($Raw) { $launchSettings.renderAgentMarkdown = $false }
            $script:app = Start-Terminal -Package (Get-ItTestPackage) -Backup $false -CleanSettings $false `
                -PassFre $true -Settings $launchSettings
            Open-AgentPane -App $script:app | Out-Null
            Wait-AgentReady -App $script:app -TimeoutSec 30 |
                Should -BeTrue -Because 'the local deterministic ACP fixture must connect'
            $script:session = Get-AgentPaneSession -App $script:app
            $script:session | Should -Not -BeNullOrEmpty
            $script:paneId = $script:session.PaneSessionId
        }

        function Get-MarkdownFrame {
            Get-WtCapture -App $script:app -SessionId $script:paneId -MaxLines 250
        }

        function Wait-MarkdownFrame {
            param(
                [Parameter(Mandatory)][string]$Pattern,
                [string]$NotPattern,
                [string]$Because = 'the Markdown display snapshot'
            )
            Wait-Until -TimeoutSec 20 -IntervalSec 0.2 -Because $Because -Condition {
                $frame = Get-MarkdownFrame
                if ($frame -match $Pattern -and (-not $NotPattern -or $frame -notmatch $NotPattern)) { $frame }
            }
        }

        function Wait-MarkdownComplete {
            param(
                [Parameter(Mandatory)][string]$Scenario,
                [Parameter(Mandatory)][string]$Marker
            )
            $pattern = '\|markdown-complete\|' + [regex]::Escape($script:session.AcpSessionId) + "\|$Scenario$"
            Wait-Until -TimeoutSec 20 -Because "the fixture to finish $Scenario" -Condition {
                if (Test-Path -LiteralPath $script:requestLog) {
                    Get-Content -LiteralPath $script:requestLog | Where-Object { $_ -match $pattern }
                }
            } | Out-Null
            $ready = Get-WtaLocalizedTextRegex -Key 'input.placeholder.connected'
            $ready | Should -Not -BeNullOrEmpty
            $completedHeader = '[\u25bc\u25b6]\s*>\s*MARKDOWN_' + $Scenario + '\b'
            Wait-Until -TimeoutSec 20 -IntervalSec 0.2 `
                -Because 'the completed-turn header, full reply, and connected input to be visible together' -Condition {
                    $frame = Get-MarkdownFrame
                    if ($frame -match $completedHeader -and $frame -match [regex]::Escape($Marker) -and $frame -match $ready) {
                        $frame
                    }
                }
        }

        function Assert-MarkdownSample {
            param([Parameter(Mandatory)][string]$Frame, [switch]$Raw)

            foreach ($marker in @('MDHEADING', 'MDBOLD', 'MDINLINE', 'MDLISTONE', 'MDLISTTWO',
                    'MDCODE', 'MDCOL', 'MDVALUE', 'MDCELL', 'MDTAIL', 'MDUNICODE', 'MDEND')) {
                [regex]::Matches($Frame, "\b$marker\b").Count | Should -Be 1 `
                    -Because "$marker must survive exactly once in the displayed reply"
            }
            $Frame | Should -Match ([regex]::Escape($script:unicode))
            foreach ($source in @('# MDHEADING', '**MDBOLD**', '`MDINLINE`', '```text', '| --- | --- |')) {
                if ($Raw) { $Frame | Should -Match ([regex]::Escape($source)) }
                else { $Frame | Should -Not -Match ([regex]::Escape($source)) }
            }
            $Frame | Should -Match ([regex]::Escape('# MDCODE **KEEP**')) `
                -Because 'fenced code content must remain literal in either mode'
            $Frame | Should -Match ([regex]::Escape('**USERLITERAL**')) `
                -Because 'user prompts must never be interpreted as agent Markdown'
        }

        function Get-MarkdownIdentity {
            $sessions = @(Get-AgentPaneSessions -App $script:app)
            $sessions | Should -HaveCount 1
            $descendants = @(Get-DescendantWtaIds -RootPid ([int]$script:app.Pid))
            $masters = @(Get-CimInstance Win32_Process -Filter "Name='wta.exe'" |
                Where-Object {
                    [int]$_.ProcessId -in $descendants -and
                    $_.CommandLine -match '--master(\s|$|")' -and
                    $_.CommandLine -notmatch '--connect-master'
                })
            $masters | Should -HaveCount 1
            $records = @(Get-Content -LiteralPath $script:requestLog)
            # Settings may probe models with another fixture process. Pin the live pane's session.
            $sessionPattern = [regex]::Escape([string]$sessions[0].AcpSessionId)
            $creation = @($records | Where-Object { $_ -match "\|session/new\|$sessionPattern(?:\||$)" })
            $creation | Should -HaveCount 1
            $agentPid = [int]$creation[0].Split('|')[0]
            $ownedRecords = @($records | Where-Object { $_ -match "^$agentPid\|" })
            $initializations = @($ownedRecords | Where-Object { $_ -match '\|initialize$' })
            $initializations | Should -HaveCount 1
            @($ownedRecords | Where-Object { $_ -match "\|session/close\|$sessionPattern(?:\||$)" }) | Should -HaveCount 0
            [pscustomobject]@{
                Pane = $sessions[0].PaneSessionId
                Helper = [int]$sessions[0].HelperProcessId
                Master = [int]$masters[0].ProcessId
                Agent = $agentPid
                Session = $sessions[0].AcpSessionId
            }
        }

        function Assert-MarkdownStreamFrame {
            param(
                [Parameter(Mandatory)][string]$Frame,
                [Parameter(Mandatory)][string]$Stage
            )
            $previous = -1
            foreach ($marker in @('MDSTREAM', 'MDOPEN', 'MDPRETOOL', 'MDTOOL', 'MDTOOLOUT', 'MDAFTER', 'MDFINAL')) {
                [regex]::Matches($Frame, "\b$marker\b").Count | Should -Be 1 `
                    -Because "$marker must appear exactly once in the $Stage display"
                $index = $Frame.IndexOf($marker, [StringComparison]::Ordinal)
                $index | Should -BeGreaterThan $previous -Because 'text and non-text updates must preserve ACP arrival order'
                $previous = $index
            }
            foreach ($literal in @('**USERLITERAL**', '**MDTOOL**', '`MDTOOLOUT`')) {
                $Frame | Should -Match ([regex]::Escape($literal))
            }
            $Frame | Should -Not -Match '# MDSTREAM|\*\*MDOPEN|# MDAFTER'
        }

        function Assert-MarkdownIdentity {
            param([Parameter(Mandatory)]$Before, [Parameter(Mandatory)][int]$Prompts)
            $after = Get-MarkdownIdentity
            foreach ($key in @('Pane', 'Helper', 'Master', 'Agent', 'Session')) {
                $after.$key | Should -Be $Before.$key -Because "display-only changes must preserve $key identity"
            }
            $sessionPattern = [regex]::Escape([string]$before.Session)
            $requests = @(Get-Content -LiteralPath $script:requestLog |
                Where-Object { $_ -match "^$($before.Agent)\|session/prompt\|$sessionPattern\|" })
            $requests | Should -HaveCount $Prompts
            foreach ($request in $requests) {
                $request.Split('|')[2] | Should -Be $Before.Session -Because 'no request may rebind to a new ACP session'
            }
        }

        function Get-MarkdownCell {
            param([Parameter(Mandatory)][string]$Frame, [Parameter(Mandatory)][string]$Text)
            $lines = $Frame -split "`r?`n"
            $hits = @(
                for ($row = 0; $row -lt $lines.Count; $row++) {
                    $column = $lines[$row].IndexOf($Text, [StringComparison]::Ordinal)
                    if ($column -ge 0) { [pscustomobject]@{ Row = $row; Column = $column } }
                }
            )
            $hits | Should -HaveCount 1 -Because "$Text must identify one current rendered row"
            $hits[0]
        }

        function Get-MarkdownWidth {
            param([Parameter(Mandatory)][string]$Frame)
            [int](($Frame -split "`r?`n" | ForEach-Object Length | Measure-Object -Maximum).Maximum)
        }

        function Release-MarkdownStage {
            param([Parameter(Mandatory)][string]$Stage)
            $pattern = '\|markdown-stage\|' + [regex]::Escape($script:session.AcpSessionId) + "\|$Stage$"
            Wait-Until -TimeoutSec 15 -Because "the fixture to reach the $Stage handoff" -Condition {
                Get-Content -LiteralPath $script:requestLog | Where-Object { $_ -match $pattern }
            } | Out-Null
            $path = "$script:requestLog.markdown-$Stage.release"
            New-Item -ItemType File -Path $path | Out-Null
        }
    }

    BeforeEach {
        $script:app = $null
        $script:backupApp = $null
        $script:streaming = $false
        $script:requestLog = Join-Path $env:TEMP "ite2e-markdown-$([guid]::NewGuid().ToString('N')).log"
        $script:gatePaths = @('partial', 'balanced', 'finish' | ForEach-Object {
            "$script:requestLog.markdown-$_.release"
        })
    }
    AfterEach {
        try {
            if ($script:streaming -and (Test-Path -LiteralPath $script:requestLog)) {
                # Drain a fixture held at an assertion that failed, so it can read
                # stdin EOF during normal ItE2E teardown rather than linger.
                foreach ($path in $script:gatePaths) {
                    if (-not (Test-Path -LiteralPath $path)) { New-Item -ItemType File -Path $path | Out-Null }
                }
                Wait-Until -TimeoutSec 10 -Because 'the gated fixture to finish before teardown' -Condition {
                    Get-Content -LiteralPath $script:requestLog | Where-Object { $_ -match '\|markdown-complete\|.*\|STREAM$' }
                } | Out-Null
            }
        }
        finally {
            try {
                if ($script:app) { Stop-Terminal -App $script:app -RestoreSettings $false }
            }
            finally {
                if ($script:backupApp) { Restore-WtConfig -App $script:backupApp }
                foreach ($path in @($script:requestLog) + @($script:gatePaths)) {
                    if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force }
                }
            }
        }
    }

    It 'Agent Markdown is enabled when the setting is absent' {
        Start-MarkdownTerminal
        (Get-WtSettingsObject -App $script:app).PSObject.Properties.Name |
            Should -Not -Contain 'renderAgentMarkdown'
        $helper = Get-CimInstance Win32_Process -Filter "ProcessId=$($script:session.HelperProcessId)"
        $helper.CommandLine | Should -Not -Match '--no-agent-markdown'
        Send-AgentPrompt -App $script:app -PaneSessionId $script:paneId -Text 'MARKDOWN_SAMPLE **USERLITERAL**' | Out-Null
        $frame = Wait-MarkdownComplete -Scenario SAMPLE -Marker MDEND
        Assert-MarkdownSample -Frame $frame
    }

    It 'Agent Markdown can be disabled before helper startup' {
        Start-MarkdownTerminal -Raw
        $helper = Get-CimInstance Win32_Process -Filter "ProcessId=$($script:session.HelperProcessId)"
        $helper.CommandLine | Should -Match '--no-agent-markdown(?:\s|$|")'
        Send-AgentPrompt -App $script:app -PaneSessionId $script:paneId -Text 'MARKDOWN_SAMPLE **USERLITERAL**' | Out-Null
        $frame = Wait-MarkdownComplete -Scenario SAMPLE -Marker MDEND
        Assert-MarkdownSample -Frame $frame -Raw
    }

    It 'CRLF Markdown preserves literal blank lines across the ACP boundary' {
        Start-MarkdownTerminal
        Send-AgentPrompt -App $script:app -PaneSessionId $script:paneId -Text 'MARKDOWN_CRLF' | Out-Null
        $frame = Wait-MarkdownComplete -Scenario CRLF -Marker MDCRLFEND
        $before = Get-MarkdownIdentity
        $frame | Should -Not -Match '```unknown'
        foreach ($kind in @('CODE', 'HTML')) {
            $first = Get-MarkdownCell -Frame $frame -Text "MDCRLF${kind}FIRST"
            $last = Get-MarkdownCell -Frame $frame -Text "MDCRLF${kind}LAST"
            ($last.Row - $first.Row) | Should -Be 2 `
                -Because 'one original blank line must remain, not disappear or be duplicated'
        }

        Set-WtSetting -App $script:app -Key renderAgentMarkdown -Value $false | Out-Null
        $raw = Wait-MarkdownFrame -Pattern '```unknown'
        foreach ($kind in @('CODE', 'HTML')) {
            $first = Get-MarkdownCell -Frame $raw -Text "MDCRLF${kind}FIRST"
            $last = Get-MarkdownCell -Frame $raw -Text "MDCRLF${kind}LAST"
            ($last.Row - $first.Row) | Should -Be 2
        }
        Assert-MarkdownIdentity -Before $before -Prompts 1
    }

    It 'Markdown Settings toggle is accessible and updates the existing reply' {
        Start-MarkdownTerminal
        Send-AgentPrompt -App $script:app -PaneSessionId $script:paneId -Text 'MARKDOWN_SAMPLE **USERLITERAL**' | Out-Null
        Assert-MarkdownSample -Frame (Wait-MarkdownComplete -Scenario SAMPLE -Marker MDEND)
        $before = Get-MarkdownIdentity

        # SplitButton Invoke opens a new tab; Expand opens its menu without depending on foreground keys.
        Add-Type -AssemblyName UIAutomationClient
        Add-Type -AssemblyName UIAutomationTypes
        $root = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$script:app.Hwnd)
        $buttonCondition = [System.Windows.Automation.PropertyCondition]::new(
            [System.Windows.Automation.AutomationElement]::AutomationIdProperty, 'NewTabButton')
        $button = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $buttonCondition)
        $button | Should -Not -BeNullOrEmpty
        $expand = [System.Windows.Automation.ExpandCollapsePattern]$button.GetCurrentPattern(
            [System.Windows.Automation.ExpandCollapsePattern]::Pattern)
        $expand.Expand()
        $menuCondition = [System.Windows.Automation.PropertyCondition]::new(
            [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
            [System.Windows.Automation.ControlType]::MenuItem)
        $settingsName = Get-WtReswTextRegex -Key 'SettingsMenuItem'
        $settingsName | Should -Not -BeNullOrEmpty
        $settingsItem = Wait-Until -TimeoutSec 10 -Because 'the localized Settings menu item' -Condition {
            $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $menuCondition) |
                Where-Object { $_.Current.Name -match "^($settingsName)$" } |
                Select-Object -First 1
        }
        $invoke = [System.Windows.Automation.InvokePattern]$settingsItem.GetCurrentPattern(
            [System.Windows.Automation.InvokePattern]::Pattern)
        $invoke.Invoke()
        Wait-UiElement -App $script:app -Selector 'SettingsNav' | Out-Null
        Invoke-SettingsNav -App $script:app -NavItem 'AIAgentsNavItem' | Out-Null
        Wait-UiElement -App $script:app -Selector 'RenderAgentMarkdownToggle' | Out-Null
        $toggle = Get-UiElement -App $script:app -Selector 'RenderAgentMarkdownToggle'
        $toggle | Should -Not -BeNullOrEmpty
        $toggle.toggleState | Should -Be 'on'
        $toggle.isEnabled | Should -BeTrue
        $name = Get-WtReswTextRegex -Key 'AIAgents_RenderAgentMarkdown.Header'
        $name | Should -Not -BeNullOrEmpty
        $toggle.name | Should -Match $name

        Invoke-UiElement -App $script:app -Selector 'RenderAgentMarkdownToggle' | Out-Null
        Invoke-UiElement -App $script:app -Selector 'SaveButton' | Out-Null
        Wait-Until -TimeoutSec 10 -Because 'the Settings toggle to persist explicit false' -Condition {
            $settings = Get-WtSettingsObject -App $script:app
            $settings.PSObject.Properties.Name -contains 'renderAgentMarkdown' -and
                $settings.renderAgentMarkdown -eq $false
        } | Should -BeTrue
        (Get-WtSettingsObject -App $script:app).acpAgent | Should -Be 'custom:markdown-fixture' `
            -Because 'saving the display preference must not change the selected provider'
        (Get-WtPaneStatus -App $script:app -SessionId $script:paneId).state | Should -Match 'run' `
            -Because 'the original helper must remain live while Settings is open'

        Invoke-WtCli -App $script:app -Arguments @('focus-pane', '-t', $script:paneId) | Out-Null
        Open-AgentPane -App $script:app | Out-Null
        Assert-MarkdownIdentity -Before $before -Prompts 1
        $raw = Wait-MarkdownFrame -Pattern '\*\*MDBOLD\*\*'
        Assert-MarkdownSample -Frame $raw -Raw
        Assert-MarkdownIdentity -Before $before -Prompts 1
    }

    It 'Live Markdown toggles preserve the conversation and process identities' {
        Start-MarkdownTerminal -Raw
        Send-AgentPrompt -App $script:app -PaneSessionId $script:paneId -Text 'MARKDOWN_SAMPLE **USERLITERAL**' | Out-Null
        Assert-MarkdownSample -Frame (Wait-MarkdownComplete -Scenario SAMPLE -Marker MDEND) -Raw
        $before = Get-MarkdownIdentity

        Set-WtSetting -App $script:app -Key renderAgentMarkdown -Value $true | Out-Null
        $styled = Wait-MarkdownFrame -Pattern 'MDEND' -NotPattern '\*\*MDBOLD\*\*'
        Assert-MarkdownSample -Frame $styled
        Assert-MarkdownIdentity -Before $before -Prompts 1

        Set-WtSetting -App $script:app -Key renderAgentMarkdown -Value $false | Out-Null
        $raw = Wait-MarkdownFrame -Pattern '\*\*MDBOLD\*\*'
        Assert-MarkdownSample -Frame $raw -Raw
        Assert-MarkdownIdentity -Before $before -Prompts 1

        # An independent hot setting emits a partial agent_config_changed event.
        # Observe consumption in THIS helper before asserting the negative control.
        Initialize-LogOffsets -App $script:app | Out-Null
        Set-WtSetting -App $script:app -Key autoFixEnabled -Value $true | Out-Null
        Assert-Log -App $script:app -Name "wta-main_helper-$($before.Helper).log" `
            -Pattern 'autofix_enabled hot-reloaded from settings change' -TimeoutSec 20
        Assert-MarkdownSample -Frame (Get-MarkdownFrame) -Raw
        Send-AgentPrompt -App $script:app -PaneSessionId $script:paneId -Text 'MARKDOWN_PING' | Out-Null
        Wait-MarkdownComplete -Scenario PING -Marker MDSECOND | Should -Match '# MDSECOND'
        Assert-MarkdownIdentity -Before $before -Prompts 2
    }

    It 'Expanded Markdown history keeps table geometry after narrow resize' {
        Start-MarkdownTerminal
        Send-AgentPrompt -App $script:app -PaneSessionId $script:paneId -Text 'MARKDOWN_TABLE' | Out-Null
        $frame = Wait-MarkdownComplete -Scenario TABLE -Marker MDTABLEEND
        $before = Get-MarkdownIdentity
        $header = Get-MarkdownCell -Frame $frame -Text MARKDOWN_TABLE
        Send-AgentMouseClick -App $script:app -PaneSessionId $script:paneId -Row $header.Row -Column $header.Column | Out-Null
        Wait-MarkdownFrame -Pattern MARKDOWN_TABLE -NotPattern MDCOPY | Out-Null
        Send-AgentKey -App $script:app -PaneSessionId $script:paneId -Key Enter | Out-Null
        $expanded = Wait-MarkdownFrame -Pattern MDTABLEEND
        $expanded | Should -Not -Match '# MDTABLE'
        $wideWidth = Get-MarkdownWidth -Frame $expanded
        $wideTableHeight = (Get-MarkdownCell -Frame $expanded -Text MDTABLEEND).Row -
            (Get-MarkdownCell -Frame $expanded -Text MDCOPY).Row

        Set-AgentPaneFocus -App $script:app | Out-Null
        $targetWidth = [Math]::Min(46, $wideWidth - 12)
        Add-Type -AssemblyName UIAutomationClient
        Add-Type -AssemblyName UIAutomationTypes
        $window = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$script:app.Hwnd)
        $bounds = $window.Current.BoundingRectangle
        $windowPattern = [System.Windows.Automation.WindowPattern]$window.GetCurrentPattern(
            [System.Windows.Automation.WindowPattern]::Pattern)
        $windowPattern.SetWindowVisualState([System.Windows.Automation.WindowVisualState]::Normal)
        $transform = [System.Windows.Automation.TransformPattern]$window.GetCurrentPattern(
            [System.Windows.Automation.TransformPattern]::Pattern)
        $transform.Resize([Math]::Max(1, [Math]::Floor($bounds.Width * $targetWidth / $wideWidth)), $bounds.Height)
        $narrow = Wait-Until -TimeoutSec 20 -IntervalSec 0.2 -Because 'the real right-hand agent pane to become narrower' -Condition {
            $capture = Get-MarkdownFrame
            if ((Get-MarkdownWidth -Frame $capture) -le $targetWidth -and $capture -match 'MDTABLEEND') {
                $capture
            }
        }
        $narrowTableHeight = (Get-MarkdownCell -Frame $narrow -Text MDTABLEEND).Row -
            (Get-MarkdownCell -Frame $narrow -Text MDCOPY).Row
        $narrowTableHeight | Should -BeGreaterOrEqual $wideTableHeight
        foreach ($word in @('MDCOPY', 'MDWIDE', 'alpha', 'beta', 'gamma', 'delta', 'epsilon', 'zeta', 'eta', 'theta', 'MDTABLEEND')) {
            [regex]::Matches($narrow, "\b$word\b").Count | Should -Be 1
        }
        $narrow | Should -Match ([regex]::Escape("`u{754c}`u{754c} caf`u{e9} e`u{301}"))
        $narrow | Should -Not -Match '\|\s*---\s*\|'

        $clipboard = Get-ClipboardSnapshot
        try {
            # The target is in the first column, before any wide-cell content.
            # Coordinates come from the post-resize snapshot, never absolute pixels.
            $cell = Get-MarkdownCell -Frame $narrow -Text MDCOPY
            Set-Clipboard -Value 'markdown-copy-sentinel'
            Send-AgentMouseClick -App $script:app -PaneSessionId $script:paneId `
                -Column $cell.Column -Row $cell.Row -Count 2 | Out-Null
            Send-AgentWin32Key -App $script:app -PaneSessionId $script:paneId -Vk 0x43 -Sc 0x2e -Uc 3 -Modifiers 0x08 | Out-Null
            Wait-Until -TimeoutSec 5 -Because 'selection to copy the displayed table cell' -Condition {
                (Get-Clipboard -Raw) -eq 'MDCOPY'
            } | Should -BeTrue
        }
        finally { Restore-ClipboardSnapshot -Snapshot $clipboard }

        $header = Get-MarkdownCell -Frame (Get-MarkdownFrame) -Text MARKDOWN_TABLE
        Send-AgentMouseClick -App $script:app -PaneSessionId $script:paneId -Row $header.Row -Column $header.Column | Out-Null
        Wait-MarkdownFrame -Pattern MARKDOWN_TABLE -NotPattern MDCOPY | Out-Null
        Send-AgentKey -App $script:app -PaneSessionId $script:paneId -Key Enter | Out-Null
        Wait-MarkdownFrame -Pattern MDTABLEEND | Should -Match 'MDCOPY'
        Assert-MarkdownIdentity -Before $before -Prompts 1
    }

    It 'Streaming Markdown reclassifies partial syntax before ordered tool output' {
        Start-MarkdownTerminal
        $script:streaming = $true
        Send-AgentPrompt -App $script:app -PaneSessionId $script:paneId -Text 'MARKDOWN_STREAM **USERLITERAL**' | Out-Null
        $partial = Wait-MarkdownFrame -Pattern '\*\*MDOPEN' -Because 'the incomplete bold prefix to remain visible'
        $partial | Should -Match 'MDSTREAM'
        $partial | Should -Not -Match '# MDSTREAM|MDTOOL|MDFINAL'
        Release-MarkdownStage -Stage partial

        $balanced = Wait-MarkdownFrame -Pattern MDPRETOOL -NotPattern '\*\*MDOPEN' `
            -Because 'closing emphasis to reclassify the visible prefix before completion'
        [regex]::Matches($balanced, '\bMDOPEN\b').Count | Should -Be 1
        $balanced | Should -Not -Match 'MDTOOL|MDFINAL'
        Release-MarkdownStage -Stage balanced

        $ordered = Wait-MarkdownFrame -Pattern MDFINAL
        Assert-MarkdownStreamFrame -Frame $ordered -Stage streaming
        Release-MarkdownStage -Stage finish
        $final = Wait-MarkdownComplete -Scenario STREAM -Marker MDFINAL
        Assert-MarkdownStreamFrame -Frame $final -Stage completed
    }

    It 'Narrow Markdown tables keep bordered columns and wrap cell text' {
        function Assert-WrappedGrid {
            param([Parameter(Mandatory)][string]$Frame, [Parameter(Mandatory)][string]$Stage,
                [bool]$CheckRowSeparators = $true)
            $lines = @($Frame -split "`r?`n")
            $top = @($lines | Where-Object { $_ -match '┌─+┬─+┐' })
            $bottom = @($lines | Where-Object { $_ -match '└─+┴─+┘' })
            $top | Should -HaveCount 1 -Because "the $Stage table must remain a grid rather than a vertical cell list"
            $bottom | Should -HaveCount 1
            $lines = $lines[[array]::IndexOf($lines, $top[0])..[array]::IndexOf($lines, $bottom[0])]
            $rules = @($lines | Where-Object { $_ -match '├[─┼]+┤' })
            if ($CheckRowSeparators) {
                $rules | Should -HaveCount 3 -Because 'the header and each logical data row need a horizontal boundary'
            }
            $gridWidth = [regex]::Match($top[0], '┌[─┬]+┐').Length
            foreach ($rule in $rules) {
                [regex]::Match($rule, '├[─┼]+┤').Length | Should -Be $gridWidth
            }
            [regex]::Match($bottom[0], '└[─┴]+┘').Length | Should -Be $gridWidth
            $cells = @($lines | Where-Object { $_ -match '│' })
            foreach ($line in $cells) { [regex]::Matches($line, '│').Count | Should -Be 3 }
            $values = ($cells | ForEach-Object { ($_ -split '│')[2] }) -join ''
            $values = $values -replace '\s', ''
            $values | Should -Be ("Descriptionalphabetagammadeltaepsilonzetaetatheta" +
                "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyz" +
                "`u{754c}`u{754c}caf`u{e9}e`u{301}`u{1f469}`u{200d}`u{1f4bb}")
            $keys = (($cells | ForEach-Object { ($_ -split '│')[1] }) -join '') -replace '\s', ''
            $keys | Should -Be 'KeyMDCOPYMDLONGMDWIDE'
            $Frame | Should -Not -Match '(?m)^\s*\[1\]\s'
            [pscustomobject]@{ Width = $gridWidth; ContentRows = $cells.Count }
        }

        function Save-GridEvidence {
            param([string]$Frame, [string]$Stage)
            if ($env:ITE2E_ARTIFACT_ROOT) {
                New-Item -ItemType Directory -Path $env:ITE2E_ARTIFACT_ROOT -Force | Out-Null
                $Frame | Set-Content -LiteralPath (Join-Path $env:ITE2E_ARTIFACT_ROOT "$Stage.txt") -Encoding utf8
                Save-UiScreenshot -App $script:app -Path (Join-Path $env:ITE2E_ARTIFACT_ROOT "$Stage.png") | Out-Null
            }
        }

        Start-MarkdownTerminal
        Send-AgentPrompt -App $script:app -PaneSessionId $script:paneId -Text 'MARKDOWN_GRID' | Out-Null
        $wide = Wait-MarkdownComplete -Scenario GRID -Marker MDGRIDEND
        $before = Get-MarkdownIdentity
        Save-GridEvidence -Frame $wide -Stage wide
        $wideGrid = Assert-WrappedGrid -Frame $wide -Stage wide -CheckRowSeparators $false
        $wideWidth = Get-MarkdownWidth -Frame $wide

        Add-Type -AssemblyName UIAutomationClient
        Add-Type -AssemblyName UIAutomationTypes
        $window = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$script:app.Hwnd)
        $windowPattern = [System.Windows.Automation.WindowPattern]$window.GetCurrentPattern(
            [System.Windows.Automation.WindowPattern]::Pattern)
        $windowPattern.SetWindowVisualState([System.Windows.Automation.WindowVisualState]::Normal)
        $bounds = $window.Current.BoundingRectangle
        $transform = [System.Windows.Automation.TransformPattern]$window.GetCurrentPattern(
            [System.Windows.Automation.TransformPattern]::Pattern)
        $targetWidth = [Math]::Min(38, $wideWidth - 12)
        $transform.Resize([Math]::Max(1, [Math]::Floor($bounds.Width * $targetWidth / $wideWidth)), $bounds.Height)
        $narrow = Wait-Until -TimeoutSec 20 -IntervalSec 0.2 -Because 'the real pane to narrow while retaining the full response' -Condition {
            $frame = Get-MarkdownFrame
            if ((Get-MarkdownWidth -Frame $frame) -le $targetWidth -and $frame -match 'MDGRIDEND') { $frame }
        }
        Save-GridEvidence -Frame $narrow -Stage narrow
        $narrowGrid = Assert-WrappedGrid -Frame $narrow -Stage narrow
        $narrowGrid.ContentRows | Should -BeGreaterThan $wideGrid.ContentRows
        $narrowGrid.Width | Should -BeLessThan $wideGrid.Width

        $clipboard = Get-ClipboardSnapshot
        try {
            $cell = Get-MarkdownCell -Frame $narrow -Text MDCOPY
            Set-Clipboard -Value 'grid-copy-sentinel'
            Send-AgentMouseClick -App $script:app -PaneSessionId $script:paneId `
                -Row $cell.Row -Column $cell.Column -Count 2 | Out-Null
            Send-AgentWin32Key -App $script:app -PaneSessionId $script:paneId -Vk 0x43 -Sc 0x2e -Uc 3 -Modifiers 0x08 | Out-Null
            Wait-Until -TimeoutSec 5 -Because 'selection to use the wrapped grid coordinates' -Condition {
                (Get-Clipboard -Raw) -eq 'MDCOPY'
            } | Should -BeTrue
        }
        finally { Restore-ClipboardSnapshot -Snapshot $clipboard }

        $windowPattern.SetWindowVisualState([System.Windows.Automation.WindowVisualState]::Maximized)
        $restored = Wait-Until -TimeoutSec 20 -IntervalSec 0.2 -Because 'widening to restore the original table geometry' -Condition {
            $frame = Get-MarkdownFrame
            if ((Get-MarkdownWidth -Frame $frame) -eq $wideWidth -and $frame -match 'MDGRIDEND') { $frame }
        }
        Save-GridEvidence -Frame $restored -Stage restored
        $restoredGrid = Assert-WrappedGrid -Frame $restored -Stage restored
        $restoredGrid.Width | Should -Be $wideGrid.Width
        $restoredGrid.ContentRows | Should -Be $wideGrid.ContentRows
        Assert-MarkdownIdentity -Before $before -Prompts 1
    }
}
