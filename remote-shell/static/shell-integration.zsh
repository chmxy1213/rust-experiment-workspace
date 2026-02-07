# Remote Shell Integration Script for Zsh

__rs_in_execution=""

__rs_precmd_zsh() {
    local ret="$?"
    if [ -n "$__rs_in_execution" ]; then
        printf "\033]6973;END;%d\007" "$ret"
        __rs_in_execution=""
    fi
}

__rs_preexec_zsh() {
    if [ -z "$__rs_in_execution" ]; then
        __rs_in_execution="yes"
        printf "\033]6973;START\007"
    fi
}

# Zsh hook arrays
# Ensure we don't duplicate if sourced multiple times (simple check)
if [[ ${precmd_functions[(ie)__rs_precmd_zsh]} -gt ${#precmd_functions} ]]; then
    precmd_functions+=("__rs_precmd_zsh")
fi
if [[ ${preexec_functions[(ie)__rs_preexec_zsh]} -gt ${#preexec_functions} ]]; then
    preexec_functions+=("__rs_preexec_zsh")
fi
