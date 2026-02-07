# Remote Shell Integration Script for Bash

__rs_in_execution=""

__rs_precmd_bash() {
    local ret="$?"
    if [ -n "$__rs_in_execution" ]; then
        printf "\033]6973;END;%d\007" "$ret"
        __rs_in_execution=""
    fi
}

__rs_preexec_bash() {
    if [ "$BASH_COMMAND" != "__rs_precmd_bash" ]; then
        if [ -z "$__rs_in_execution" ]; then
            __rs_in_execution="yes"
            # Format: START;USER;HOSTNAME;PWD
            printf "\033]6973;START;%s;%s;%s\007" "$USER" "$HOSTNAME" "$PWD"
        fi
    fi
}

PROMPT_COMMAND=__rs_precmd_bash
trap '__rs_preexec_bash' DEBUG
