# Remote Shell Integration Script for Zsh

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
        print -n "\033]6973;START\007"
    fi
}

# Zsh hook arrays
# Clear existing hooks if they are ours to prevent duplication issues during reload
precmd_functions=(${precmd_functions:#__rs_precmd_zsh})
preexec_functions=(${preexec_functions:#__rs_preexec_zsh})

precmd_functions+=("__rs_precmd_zsh")
preexec_functions+=("__rs_preexec_zsh")

