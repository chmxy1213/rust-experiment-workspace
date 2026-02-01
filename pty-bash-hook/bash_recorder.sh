#!/bin/bash

# 加载用户默认配置 (模拟交互式 Shell 行为)
if [ -f "$HOME/.bashrc" ]; then
    source "$HOME/.bashrc"
elif [ -f "$HOME/.profile" ]; then
    source "$HOME/.profile"
fi

# ==============================================================================
# Bash PTY Integration Probe
# 适用场景: 用户进程创建 PTY -> PTY 运行 Bash
# 功能: 通过 stdout 发送不可见的 OSC 序列，向宿主进程汇报命令状态
# ==============================================================================

# ------------------------------------------------------------------------------
# 通信协议: ANSI OSC (Operating System Command)
# 格式: \033]666;<TYPE>;<PAYLOAD>\007
# 宿主进程需解析此序列以获取元数据，且不应将其显示在终端界面上
# ------------------------------------------------------------------------------

__pty_send_signal() {
    local type="$1"
    local payload="$2"
    
    # 对 payload 进行 Base64 编码，防止包含特殊字符（如换行、分号）破坏协议格式
    # 如果宿主进程处理简单，也可以不编码，但要小心特殊字符
    # 这里为了演示简单，直接发送原始内容。如果内容复杂，建议由宿主进程处理流清洗。
    
    #使用 builtin printf 确保性能和行为一致
    builtin printf "\033]666;%s;%s\007" "$type" "$payload"
}

# ------------------------------------------------------------------------------
# 钩子函数
# ------------------------------------------------------------------------------

# 1. 命令执行前 (Pre-exec)
__pty_preexec() {
    # 避免在命令补全时触发
    if [ -n "$COMP_LINE" ]; then return; fi
    
    local this_command="$BASH_COMMAND"
    
    # 忽略钩子自身的调用
    if [[ "$this_command" == "__pty_precmd" ]]; then return; fi
    
    # 发送 CMD_START 信号，附带具体的命令文本
    __pty_send_signal "CMD_START" "$this_command"
}

# 2. 命令执行后 (Pre-cmd / Prompt)
__pty_precmd() {
    local exit_code="$?"
    
    # 发送 CMD_END 信号，附带退出码
    __pty_send_signal "CMD_END" "$exit_code"
    
    # (可选) 发送当前路径
    __pty_send_signal "PWD" "$PWD"
}

# ------------------------------------------------------------------------------
# 安装钩子
# ------------------------------------------------------------------------------

# 使用 trap DEBUG 捕获命令开始
trap '__pty_preexec' DEBUG

# 使用 PROMPT_COMMAND 捕获命令结束 (在再次显示提示符之前)
# 保留用户原有的 PROMPT_COMMAND
__original_prompt_command="${PROMPT_COMMAND:-}"

# 注意：PROMPT_COMMAND 是 bash 在显示提示符之前执行的
# 所以它的执行意味着上一条命令已经结束
PROMPT_COMMAND="__pty_precmd; $__original_prompt_command"

# ------------------------------------------------------------------------------
# (可选) 提示符注入
# 如果你想让宿主进程精确知道提示符何时结束（以便区分输出和新的输入），
# 需要在 PS1 中注入类似 \[...\] 的标记。如果不需要，可忽略此段。
# ------------------------------------------------------------------------------
# PS1="\[\033]666;PROMPT_START;\007\]$PS1\[\033]666;PROMPT_END;\007\]"
