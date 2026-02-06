# ==============================================================================
# PowerShell PTY Integration Probe
# Bash recorder.sh PowerShell version
# Use case: User process creates PTY -> PTY runs pwsh
# Function: Send invisible OSC sequences via stdout to report command status
# ==============================================================================

# ------------------------------------------------------------------------------
# Communication Protocol: ANSI OSC (Operating System Command)
# Format: \033]666;<TYPE>;<PAYLOAD>\007
# Host process needs to parse this sequence to get metadata
# ------------------------------------------------------------------------------

function Send-PtySignal {
    param(
        [string]$Type,
        [string]$Payload
    )
    $esc = [char]0x1b
    $bel = [char]0x07
    $signal = $esc + ']666;' + $Type + ';' + $Payload + $bel
    [Console]::Write($signal)
}

# ------------------------------------------------------------------------------
# Hook Installation
# ------------------------------------------------------------------------------

# Global state variables
$Global:__pty_use_psreadline = $false
$Global:__pty_last_hist_id = -1

# 1. Pre-exec hook
# Use PSReadLine Enter key binding to capture user input
if (Get-Module -ListAvailable PSReadLine) {
    if (-not (Get-Module PSReadLine)) {
        Import-Module PSReadLine -ErrorAction SilentlyContinue
    }

    if (Get-Module PSReadLine) {
        $Global:__pty_use_psreadline = $true
        Set-PSReadLineKeyHandler -Key Enter -ScriptBlock {
            param($key, $arg)

            $line = $null
            $cursor = $null
            [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$line, [ref]$cursor)

            if (-not [string]::IsNullOrWhiteSpace($line)) {
                Send-PtySignal "CMD_START" $line
            }

            [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
        }
    }
} 

# 2. Post-exec hook (Pre-cmd / Prompt)
# Override prompt function

# Save original prompt function
if (Test-Path function:prompt) {
    $Global:__original_prompt_block = $function:prompt
} else {
    $Global:__original_prompt_block = { "PS $PWD> " }
}

# Define new prompt function
function Global:prompt {
    $lastStatus = $?
    $lastCode = $global:LASTEXITCODE

    if ($lastStatus) {
        $exitCode = 0
    } else {
        if ($lastCode -ne 0) {
            $exitCode = $lastCode
        } else {
            $exitCode = 1
        }
    }

    if (-not $Global:__pty_use_psreadline) {
        $recent = Get-History -Count 1
        if ($recent) {
            if ($recent.Id -gt $Global:__pty_last_hist_id) {
                Send-PtySignal "CMD_START" $recent.CommandLine
                $Global:__pty_last_hist_id = $recent.Id
            }
        }
    }

    Send-PtySignal "CMD_END" "$exitCode"
    Send-PtySignal "PWD" "$PWD"

    & $Global:__original_prompt_block
}
