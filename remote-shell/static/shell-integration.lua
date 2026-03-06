-- Remote Shell Integration for Clink (cmd.exe)
-- Emits OSC 6973 sequences for command tracking
-- Compatibility: Clink v0.4.9 (Win7 era) and Clink v1.x+

local osc_prefix = "\x1b]6973;"
local osc_suffix = "\x07"

local function emit_osc(data)
    io.write(osc_prefix .. data .. osc_suffix)
    io.flush()
end

local function get_cwd()
    return os.getcwd()
end

local function write_diag(message)
    if log and log.info then
        pcall(log.info, "[remote-shell] " .. message)
    end
end

write_diag("integration script loaded; cwd=" .. tostring(get_cwd()))

-- =============================================================================
-- Clink v1.x+ API (Modern)
-- =============================================================================

if clink.onbeginedit and clink.onendedit then
    clink.onendedit(function(line)
        write_diag("onendedit fired; line=" .. tostring(line))
        if line and line:match("%S") then
            -- START signal
            local user = os.getenv("USERNAME") or "user"
            local host = os.getenv("COMPUTERNAME") or "host"
            local cwd = get_cwd()
            emit_osc("START;" .. user .. ";" .. host .. ";" .. cwd)
        end
    end)

    clink.onbeginedit(function()
        -- END signal (Post-exec)
        local exit_code = os.geterrorlevel and os.geterrorlevel() or 0
        write_diag("onbeginedit fired; exit_code=" .. tostring(exit_code))
        emit_osc("END;" .. tostring(exit_code))
    end)

    -- =============================================================================
    -- Clink v0.4.x API (Legacy / Windows 7)
    -- =============================================================================
else
    -- In v0.4.9, we lack explicit pre-exec hooks (onendedit).
    -- We can only reliably hook the prompt display (Post-exec).
    write_diag("legacy Clink API detected")

    local function legacy_prompt_filter()
        -- 1. Emit END signal for previous command
        -- v0.4.9 unfortunately makes getting the errorlevel tricky from Lua.
        -- We'll assume 0 or try to parse it if exposed, but usually it's not.
        -- We emit '0' or '?' as placeholder.
        write_diag("legacy prompt filter fired")
        emit_osc("END;0")

        -- 2. Emit START signal logic is hard here because we are at PROMPT display.
        -- The user hasn't typed anything yet.
        -- Strategy: We emit context (CWD/USER) so the backend knows where we are.
        -- But we can't emit START because we aren't starting a command.

        -- To support START in v0.4.9, we'd need to wrap 'clink.prompt.value'
        -- but that doesn't help with intercepting the Enter key.

        -- Compromise: Just emit User/Host/Cwd update effectively via a custom event if needed
        -- or just leave it. The frontend will miss the explicit START timing.

        return false -- Don't modify prompt value
    end

    clink.prompt.register_filter(legacy_prompt_filter, 99)
end
