# PowerShell Remote Shell Integration
# Emits OSC 6973 sequences for command tracking

$Global:__rs_current_command = ""

function __rs_emit_osc {
    param($data)
    $esc = [char]0x1b
    $bel = [char]0x07
    [Console]::Write("$esc]6973;$data$bel")
}

# Pre-exec via PSReadLine (if available)
$Global:__rs_has_psreadline = $false
if (Get-Module -ListAvailable PSReadLine) {
    if (-not (Get-Module PSReadLine)) {
        Import-Module PSReadLine -ErrorAction SilentlyContinue
    }
    if (Get-Module PSReadLine) {
        $Global:__rs_has_psreadline = $true
    }
}

if ($Global:__rs_has_psreadline) {
    Set-PSReadLineKeyHandler -Key Enter -ScriptBlock {
        param($key, $arg)
        
        $line = $null
        $cursor = $null
        [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$line, [ref]$cursor)
        
        # Only emit if command is not empty
        if (-not [string]::IsNullOrWhiteSpace($line)) {
            $user = $env:USERNAME
            if (-not $user) { $user = $env:USER }
            
            $hostName = $env:COMPUTERNAME
            if (-not $hostName) { $hostName = hostname }

            $pwdPath = (Get-Location).Path
            
            # Emit START
            $esc = [char]0x1b
            $bel = [char]0x07
            [Console]::Write("$esc]6973;START;$user;$hostName;$pwdPath$bel")
        }

        # Execute the command
        [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
    }
} else {
    # Legacy Fallback for Windows 7 / No PSReadLine
    # We cannot hook 'Enter' easily without PSReadLine. 
    # Instead, we emit a 'START' signal as part of the prompt, 
    # but this is less accurate because it happens BEFORE user types command.
    # Protocol Adjustment: 
    # If we emit START at prompt time, the semantic becomes "Ready specifically for new command".
    # However, to be consistent with Bash/Zsh hooks which fire AFTER enter but BEFORE output:
    # A common hack is to rely on the fact that prompt is re-evaluated.
    
    # Actually, for Legacy PowerShell, we might just have to skip explicit START signals 
    # or emit a 'PROMPT' signal that the frontend treats as a boundary.
    # For now, we will add a trivial fallback that warns or attempts basic prompt capability.
    Write-Warning "PSReadLine not found. Shell integration functionality will be limited to prompt tracking."
}

# Post-exec (Prompt hook)
# We rename the existing prompt function and call it, or just wrap it.
if (Test-Path function:\prompt) {
    # Check if we already hooked it to avoid recursion
    if (-not (Test-Path function:\__rs_original_prompt)) {
        Rename-Item function:\prompt function:\__rs_original_prompt
    }
} else {
    function __rs_original_prompt { "PS > " } # Fallback
}

function prompt {
    # Capture exit code of previous command
    # $? is True/False
    $lastSuccess = $?
    
    # 1. Emit END signal for the PREVIOUS command
    #    (Reason: prompt runs after command finishes)
    $exitCode = 0
    if (-not $lastSuccess) {
        $exitCode = 1
        if ($global:LASTEXITCODE -ne $null) {
            $exitCode = $global:LASTEXITCODE
        }
    }
    
    $esc = [char]0x1b
    $bel = [char]0x07
    [Console]::Write("$esc]6973;END;$exitCode$bel")

    # 2. If NO PSReadLine, we might emit a pseudo-START here?
    #    No, because we don't know when the user hits enter.
    #    We only know the prompt is being drawn.
    #    We can emit a PWD/USER contextual info here though.
    if (-not $Global:__rs_has_psreadline) {
       # Fallback: Emit context info so frontend at least knows where we are
       $user = $env:USERNAME
       $hostName = $env:COMPUTERNAME
       $pwdPath = (Get-Location).Path
       # We use a custom 'PROMPT' type or just reuse START with caution?
       # Let's emit a specific PROMPT event if we wanted to support it, 
       # but for now, we just leave it. The frontend might interpret 
       # text output after END as command output, which includes the typing.
       # Without PSReadLine, splitting input vs output is very hard.
    }

    # Call original prompt
    return (__rs_original_prompt)
}
