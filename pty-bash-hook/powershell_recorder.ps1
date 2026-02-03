
# ==============================================================================
# PowerShell PTY Integration Probe
# 对应 bash_recorder.sh 的 PowerShell 版本
# 适用场景: 用户进程创建 PTY -> PTY 运行 pwsh
# 功能: 通过 stdout 发送不可见的 OSC 序列，向宿主进程汇报命令状态
# ==============================================================================

# 加载用户配置 (如果需要模拟完整交互式环境)
# 注意：PowerShell 启动时通过 -File 可能会跳过部分 Profile 加载，视具体启动参数而定。
# 如果此脚本通过 powershell -NoExit -File 调用，建议保持默认 Profile 加载行为。

# ------------------------------------------------------------------------------
# 通信协议: ANSI OSC (Operating System Command)
# 格式: \033]666;<TYPE>;<PAYLOAD>\007
# 宿主进程需解析此序列以获取元数据
# ------------------------------------------------------------------------------

function Send-PtySignal {
    param(
        [string]$Type,
        [string]$Payload
    )
    # 使用 [Console]::Write 直接写入标准输出，避免 Write-Host 可能带来的换行或格式干扰
    $esc = [char]0x1b
    $bel = [char]0x07
    [Console]::Write("$esc]666;$Type;$Payload$bel")
}

# ------------------------------------------------------------------------------
# 钩子安装
# ------------------------------------------------------------------------------

# 全局状态变量
$Global:__pty_use_psreadline = $false
$Global:__pty_last_hist_id = -1

# 1. 命令执行前 (Pre-exec)
# 利用 PSReadLine 的 Enter 键绑定来捕获用户输入的命令
if (Get-Module -ListAvailable PSReadLine) {
    # 确保模块已加载
    if (-not (Get-Module PSReadLine)) {
        Import-Module PSReadLine -ErrorAction SilentlyContinue
    }

    if (Get-Module PSReadLine) {
        $Global:__pty_use_psreadline = $true
        Set-PSReadLineKeyHandler -Key Enter -ScriptBlock {
            param($key, $arg)

            $line = $null
            $cursor = $null
            # 获取当前缓冲区内容
            [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$line, [ref]$cursor)

            # 发送 CMD_START 信号
            if (-not [string]::IsNullOrWhiteSpace($line)) {
                Send-PtySignal "CMD_START" $line
            }

            # 执行原来的接受行操作
            [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
        }
    }
} 

# 2. 命令执行后 (Pre-cmd / Prompt)
# 通过覆盖 prompt 函数来实现。prompt 函数在每次命令结束后、显示提示符前执行。

# 保存原有的 prompt 函数
if (Test-Path function:prompt) {
    $Global:__original_prompt_block = $function:prompt
} else {
    # 如果没有定义 prompt，提供一个默认的最简实现
    $Global:__original_prompt_block = { "PS $PWD> " }
}

# 定义新的 prompt 函数
function Global:prompt {
    # 1. 立即捕获上一条命令的执行状态
    # $? 为 True/False，$LASTEXITCODE 为退出码（通常针对 Native 命令）
    $lastStatus = $?
    $lastCode = $global:LASTEXITCODE

    # 尝试将状态转换为类似于 Bash $? 的整数退出码
    if ($lastStatus) {
        $exitCode = 0
    } else {
        # 如果失败且 LASTEXITCODE 非 0，则使用 LASTEXITCODE
        # 否则（如 Cmdlet 错误但未设置 LASTEXITCODE），默认为 1
        if ($lastCode -ne 0) {
            $exitCode = $lastCode
        } else {
            $exitCode = 1
        }
    }

    # 兼容性处理：如果没有 PSReadLine (如 Windows 7 + PowerShell 2.0)，
    # 则尝试通过 Get-History 在命令结束后补发 CMD_START。
    # 虽然时序上晚于输出 (Post-exec)，但至少能保证宿主获取到命令文本。
    if (-not $Global:__pty_use_psreadline) {
        $recent = Get-History -Count 1
        if ($recent) {
            # 只有当 History ID 递增时才发送，避免空回车重复发送
            if ($recent.Id -gt $Global:__pty_last_hist_id) {
                # 补发 START 信号
                Send-PtySignal "CMD_START" $recent.CommandLine
                $Global:__pty_last_hist_id = $recent.Id
            }
        }
    }

    # 2. 发送信号
    # CMD_END: 发送退出码
    Send-PtySignal "CMD_END" "$exitCode"
    
    # PWD: 发送当前路径
    Send-PtySignal "PWD" "$PWD"

    # 3. 执行并返回原 prompt 的结果
    # 注意：prompt 函数必须有返回值（即提示符字符串），直接 invoke 脚本块即可
    & $Global:__original_prompt_block
}
