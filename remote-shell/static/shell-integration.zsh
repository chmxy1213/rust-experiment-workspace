# Remote Shell Integration Script for Zsh

# Disable autosuggestions (plugin)
ZSH_AUTOSUGGEST_DISABLE=true
# Unbind autosuggest widgets aggressively if needed
bindkey '^M' accept-line

# Disable syntax highlighting (plugin)
ZSH_HIGHLIGHT_HIGHLIGHTERS=()

# Disable completion menu and listings to avoid screen redraws
# Unset options that trigger menu completion or listing
unsetopt AUTO_MENU          # Don't show menu completion
unsetopt MENU_COMPLETE      # Don't automatically insert first match
unsetopt AUTO_LIST          # Don't list choices on ambiguous completion
unsetopt LIST_TYPES         # Don't show file types in completion
unsetopt ALWAYS_LAST_PROMPT # Don't return to last prompt after listing (avoids scroll/redraw)

# Disable Flow Control (Ctrl+S/-Q)
setopt NO_FLOW_CONTROL

# Disable Beep
setopt NO_BEEP

# Disable Right Prompt (RPROMPT) to avoid redraws on resize/updates
RPROMPT=""
RPS1=""

# Disable the "partial line" indicator (%) to keep logs clean
setopt no_prompt_sp

__rs_in_execution=""

__rs_precmd_zsh() {
    local ret="$?"
    if [ -n "$__rs_in_execution" ]; then
        # Use builtin print to ensure reliability and hex escape for BEL
        print -n "\033]6973;END;${ret}\007"
        __rs_in_execution=""
    fi
}

__rs_preexec_zsh() {
    if [ -z "$__rs_in_execution" ]; then
        __rs_in_execution="yes"
        # Format: START;USER;HOST;CWD
        print -n "\033]6973;START;${USER};${HOST};${PWD}\007"
    fi
}

# Zsh hook arrays
# Clear existing hooks if they are ours to prevent duplication issues during reload
precmd_functions=(${precmd_functions:#__rs_precmd_zsh})
preexec_functions=(${preexec_functions:#__rs_preexec_zsh})

precmd_functions+=("__rs_precmd_zsh")
preexec_functions+=("__rs_preexec_zsh")

