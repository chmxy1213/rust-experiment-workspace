//! Web API

use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
    thread,
};

#[cfg(windows)]
use std::path::{Path, PathBuf};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query,
    },
    response::{Html, IntoResponse},
};
use futures::{sink::SinkExt, stream::StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::command::MyCommand;
use crate::static_files::Asset;
use crate::{ClientMsg, ServerLogMsg};

#[cfg(windows)]
fn clink_binary_name() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "clink_x64.exe"
    } else if cfg!(target_arch = "x86") {
        "clink_x86.exe"
    } else if cfg!(target_arch = "aarch64") {
        "clink_arm64.exe"
    } else {
        "clink_x64.exe"
    }
}

#[cfg(windows)]
fn clink_binary_candidates() -> Vec<&'static str> {
    let mut candidates = vec![clink_binary_name(), "clink.exe", "clink.bat"];
    candidates.dedup();
    candidates
}

#[cfg(windows)]
fn clink_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            roots.push(exe_dir.to_path_buf());
            if let Some(parent) = exe_dir.parent() {
                roots.push(parent.to_path_buf());
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }

    roots
}

#[cfg(windows)]
fn clink_search_directories() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    for root in clink_search_roots() {
        dirs.push(root.join("clink"));
    }

    if let Some(path_var) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path_var));
    }

    for env_name in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(base) = std::env::var_os(env_name) {
            dirs.push(PathBuf::from(base).join("clink"));
        }
    }

    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        let base = PathBuf::from(local_app_data);
        dirs.push(base.join("Programs").join("clink"));
        dirs.push(base.join("Microsoft").join("WinGet").join("Packages"));
    }

    let mut deduped = Vec::new();
    for dir in dirs {
        if !deduped.iter().any(|existing| existing == &dir) {
            deduped.push(dir);
        }
    }

    deduped
}

#[cfg(windows)]
fn find_clink_executable() -> Option<PathBuf> {
    for dir in clink_search_directories() {
        for binary_name in clink_binary_candidates() {
            let candidate = dir.join(binary_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        let packages_dir = PathBuf::from(local_app_data)
            .join("Microsoft")
            .join("WinGet")
            .join("Packages");
        if let Ok(entries) = std::fs::read_dir(packages_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                let name = name.to_string_lossy().to_lowercase();
                if !name.contains("clink") {
                    continue;
                }

                for suffix in ["", "clink"] {
                    let base = if suffix.is_empty() {
                        path.clone()
                    } else {
                        path.join(suffix)
                    };

                    for binary_name in clink_binary_candidates() {
                        let candidate = base.join(binary_name);
                        if candidate.is_file() {
                            return Some(candidate);
                        }
                    }
                }
            }
        }
    }

    None
}

#[cfg(windows)]
fn quote_cmd_arg(arg: &Path) -> String {
    format!("\"{}\"", arg.to_string_lossy())
}

#[cfg(windows)]
fn build_clink_inject_args(profile_dir: &Path) -> Option<(String, Vec<String>)> {
    let clink_exe = find_clink_executable()?;
    tracing::info!("Using Clink executable: {}", clink_exe.display());

    let launcher_path = profile_dir.join("remote-shell-clink-inject.cmd");
    let launcher_log_path = profile_dir.join("remote-shell-clink-inject.log");
    let launcher_script = format!(
        concat!(
            "@echo off\r\n",
            ">> {} echo launcher_start %DATE% %TIME%\r\n",
            "{} inject --profile {}\r\n",
            "set REMOTE_SHELL_CLINK_INJECT_EXIT=%ERRORLEVEL%\r\n",
            ">> {} echo launcher_end %DATE% %TIME% exit=%REMOTE_SHELL_CLINK_INJECT_EXIT%\r\n"
        ),
        quote_cmd_arg(&launcher_log_path),
        quote_cmd_arg(&clink_exe),
        quote_cmd_arg(profile_dir),
        quote_cmd_arg(&launcher_log_path)
    );

    if std::fs::write(&launcher_path, launcher_script).is_err() {
        tracing::warn!(
            "Failed to write Clink launcher script: {}",
            launcher_path.display()
        );
        return None;
    }

    tracing::info!(
        "Prepared Clink launcher script: {} (log: {})",
        launcher_path.display(),
        launcher_log_path.display()
    );

    Some((
        "cmd.exe".to_string(),
        vec![
            "/d".to_string(),
            "/k".to_string(),
            launcher_path.to_string_lossy().to_string(),
        ],
    ))
}

#[derive(Deserialize)]
pub struct ConnectParams {
    pub shell: Option<String>,
}

pub async fn index_handler() -> Html<String> {
    if let Some(content) = Asset::get("index.html") {
        Html(String::from_utf8_lossy(&content.data).to_string())
    } else {
        Html("<h1>404 Not Found</h1>".to_string())
    }
}

pub async fn ws_handler(
    Query(params): Query<ConnectParams>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, params))
}

async fn handle_socket(socket: WebSocket, params: ConnectParams) {
    tracing::info!("New WebSocket connection established");
    let mut my_cmd = MyCommand::new();

    let default_shell = std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string());
    let shell = params.shell.unwrap_or(default_shell);
    let is_bash = shell.ends_with("bash");
    let is_zsh = shell.ends_with("zsh");
    let is_pwsh = shell.ends_with("pwsh") || shell.ends_with("powershell");
    let is_cmd = shell.ends_with("cmd") || shell.ends_with("cmd.exe");

    // Extract shell integration script to a temporary file
    let mut temp_script_path = None;
    let script_name = if is_bash {
        Some("shell-integration.bash")
    } else if is_zsh {
        Some("shell-integration.zsh")
    } else if is_pwsh {
        Some("shell-integration.ps1")
    } else {
        None
    };

    if let Some(name) = script_name {
        if let Some(content) = Asset::get(name) {
            let mut temp_file = tempfile::Builder::new()
                .prefix("shell-integration-")
                .suffix(if is_pwsh { ".ps1" } else { "" })
                .tempfile()
                .expect("Failed to create temp file");
            temp_file
                .write_all(&content.data)
                .expect("Failed to write temp file");
            let path = temp_file.into_temp_path();
            let path_str = path.to_string_lossy().to_string();
            temp_script_path = Some((path, path_str));
        }
    }

    // For cmd.exe on Windows: install the Lua integration script into a temporary
    // Clink profile directory. We then inject Clink into the PTY-hosted cmd.exe session.
    // The TempDir is kept alive for the duration of the WebSocket session so Clink
    // can continue to access its scripts until the connection closes.
    #[cfg(windows)]
    let mut clink_profile_dir: Option<tempfile::TempDir> = None;
    #[cfg(windows)]
    if is_cmd {
        if let Some(content) = Asset::get("shell-integration.lua") {
            if let Ok(dir) = tempfile::TempDir::new() {
                let lua_path = dir.path().join("remote-shell-integration.lua");
                let startup_cmd_path = dir.path().join("clink_start.cmd");
                let startup_cmd = concat!(
                    "@echo off\r\n",
                    ">> \"%~dp0remote-shell-clink-startup.log\" echo clink_start %%DATE%% %%TIME%%\r\n"
                );
                if std::fs::write(&lua_path, &content.data).is_ok() {
                    let _ = std::fs::write(&startup_cmd_path, startup_cmd);
                    tracing::info!(
                        "Prepared temporary Clink profile: {} (integration script: {}, startup script: {})",
                        dir.path().display(),
                        lua_path.display(),
                        startup_cmd_path.display()
                    );
                    clink_profile_dir = Some(dir);
                }
            }
        }
    }

    let mut args: Vec<String> = vec![];
    if is_bash {
        if let Some((_, ref path_str)) = temp_script_path {
            args = vec!["--rcfile".to_string(), path_str.clone()];
        }
    } else if is_pwsh {
        args = vec!["-NoLogo".to_string(), "-NoExit".to_string()];
    }

    let spawn_result = if is_cmd {
        // On Windows, Clink must be injected into the current cmd.exe session.
        // `clink.bat launch` starts a separate console window, which does not work under a PTY.
        #[cfg(windows)]
        {
            if let Some(ref dir) = clink_profile_dir {
                if let Some((cmd, args)) = build_clink_inject_args(dir.path()) {
                    tracing::info!("Launching cmd.exe with Clink injection: {}", cmd);
                    my_cmd.spawn(&cmd, &args)
                } else {
                    tracing::warn!(
                        "Clink executable not found in bundled, PATH, or common install directories; launching plain cmd.exe"
                    );
                    my_cmd.spawn("cmd.exe", &[] as &[String])
                }
            } else {
                my_cmd.spawn("cmd.exe", &[] as &[String])
            }
        }

        #[cfg(not(windows))]
        {
            my_cmd.spawn(&shell, &[] as &[String])
        }
    } else {
        my_cmd.spawn(&shell, &args)
    };

    // Fallback for pwsh -> powershell on Windows
    let mut actual_result = spawn_result;
    if actual_result.is_err() && shell == "pwsh" && cfg!(target_os = "windows") {
        tracing::warn!("Failed to spawn pwsh, falling back to powershell");
        actual_result = my_cmd.spawn(
            "powershell",
            &["-NoLogo".to_string(), "-NoExit".to_string()],
        );
    }

    let (mut reader, writer) = match actual_result {
        Ok(rw) => rw,
        Err(e) => {
            tracing::error!("Failed to spawn shell: {}", e);
            return;
        }
    };

    let writer = Arc::new(Mutex::new(writer));
    let master = Arc::new(Mutex::new(my_cmd));

    // Wait a brief moment to ensure the shell is ready to receive input before injecting commands.
    // zsh's config (like oh-my-zsh) can take a long time to load, and zle's tcsetattr(TCSAFLUSH)
    // will drop any input in the queue. Wait 1000ms to be safer.
    if is_zsh {
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    }

    // Inject scripts for shells that don't support --rcfile easily or where we prefer dynamic loading
    if is_zsh {
        if let Ok(mut w) = writer.lock() {
            if let Some((_, ref path_str)) = temp_script_path {
                // Prepend some spaces in case the first character is swallowed by a slight race
                let init_cmd = format!("   source {}\n", path_str);
                let _ = w.write_all(init_cmd.as_bytes());
                let _ = w.flush();
            }
        }
    } else if is_pwsh {
        if let Some((_, ref path_str)) = temp_script_path {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if let Ok(mut w) = writer.lock() {
                // powershell needs \r\n to correctly finish the input line properly
                let init_cmd = format!(". '{}'\r\n", path_str);
                let _ = w.write_all(init_cmd.as_bytes());
                let _ = w.flush();
            }
        }
    }

    let (tx_output, mut rx_output) = mpsc::channel::<Vec<u8>>(32);
    let (tx_log, mut rx_log) = mpsc::channel::<ServerLogMsg>(32);

    // Spawn blocking thread for reading PTY
    thread::spawn(move || {
        let mut buf = [0u8; 2048];
        let mut parser = vte::Parser::new();
        let mut interpreter = LogInterpreter::new(tx_log);

        loop {
            match reader.read(&mut buf) {
                Ok(n) if n > 0 => {
                    let data = buf[..n].to_vec();
                    // Send RAW output to frontend terminal
                    if tx_output.blocking_send(data.clone()).is_err() {
                        break;
                    }

                    // Feed data to VTE parser for log extraction
                    parser.advance(&mut interpreter, &data);
                    interpreter.flush();
                }
                Ok(_) => {
                    tracing::info!("PTY EOF");
                    break;
                }
                Err(e) => {
                    tracing::error!("PTY Read Error: {}", e);
                    break;
                }
            }
        }
        tracing::info!("PTY read thread exited");
    });

    let (mut sender, mut receiver) = socket.split();

    let send_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(data) = rx_output.recv() => {
                    if sender.send(Message::Binary(data)).await.is_err() {
                        break;
                    }
                }
                Some(log_msg) = rx_log.recv() => {
                    if let Ok(json) = serde_json::to_string(&log_msg) {
                         if sender.send(Message::Text(json)).await.is_err() {
                            break;
                         }
                    }
                }
                else => break,
            }
        }
    });

    struct LogInterpreter {
        tx_log: mpsc::Sender<ServerLogMsg>,
        capturing: bool,
        buffer: String,
    }

    impl LogInterpreter {
        fn new(tx_log: mpsc::Sender<ServerLogMsg>) -> Self {
            Self {
                tx_log,
                capturing: false,
                buffer: String::new(),
            }
        }

        fn flush(&mut self) {
            if !self.buffer.is_empty() {
                let _ = self.tx_log.blocking_send(ServerLogMsg::LogOutput {
                    data: std::mem::take(&mut self.buffer),
                });
            }
        }
    }

    impl vte::Perform for LogInterpreter {
        fn print(&mut self, c: char) {
            if self.capturing {
                self.buffer.push(c);
            }
        }

        fn execute(&mut self, byte: u8) {
            if self.capturing {
                // Handle basic control chars that are useful in logs: \n, \t, \r
                if byte == b'\n' {
                    self.buffer.push('\n');
                } else if byte == b'\t' {
                    self.buffer.push('\t');
                } else if byte == b'\r' {
                    // Ignore CR or handle it? Usually \r\n is processed.
                    // For logs, simple \n is usually enough.
                    // If we push \r, it might mess up some simple log viewers, but let's keep it safe or ignore?
                    // Let's ignore it to keep logs clean text.
                }
            }
        }

        fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
            if params.is_empty() {
                return;
            }

            // Check if code is 6973
            // params[0] like "6973"
            let code = params[0];
            if code == b"6973" {
                // Handle simple command parameter structure (params[1])
                // Cases:
                // 1. 6973;START;USER;HOST;CWD...
                // 2. 6973;END;0
                if params.len() > 1 {
                    let cmd = params[1];

                    if cmd == b"START" {
                        self.capturing = true;
                        self.buffer.clear();

                        // Parse Context: params[2]=USER, params[3]=HOST, params[4..]=CWD
                        let mut user = String::new();
                        let mut host = String::new();
                        let mut cwd = String::new();

                        if params.len() > 2 {
                            user = String::from_utf8_lossy(params[2]).to_string();
                        }
                        if params.len() > 3 {
                            host = String::from_utf8_lossy(params[3]).to_string();
                        }
                        if params.len() > 4 {
                            // Join remaining parts with ; in case CWD contained semicolons
                            let parts: Vec<String> = params[4..]
                                .iter()
                                .map(|&p| String::from_utf8_lossy(p).to_string())
                                .collect();
                            cwd = parts.join(";");
                        }

                        let _ =
                            self.tx_log
                                .blocking_send(ServerLogMsg::LogStart { user, host, cwd });
                    } else if cmd.starts_with(b"END") {
                        // Flush pending buffer first
                        self.flush();

                        let mut exit_code = 0;

                        // Try to extract exit code
                        // Case A: 6973;END;123 (Standard vte split) -> params[1]="END", params[2]="123"
                        if params.len() > 2 {
                            if let Ok(s) = std::str::from_utf8(params[2]) {
                                if let Ok(n) = s.parse::<i32>() {
                                    exit_code = n;
                                }
                            }
                        }
                        // Case B: 6973;END;123 (If vte didn't split on second semi-col for some reason, rare)
                        // Or if script sent it weirdly.
                        else if cmd.len() > 4 && cmd[3] == b';' {
                            if let Ok(s) = std::str::from_utf8(&cmd[4..]) {
                                if let Ok(n) = s.parse::<i32>() {
                                    exit_code = n;
                                }
                            }
                        }

                        let _ = self
                            .tx_log
                            .blocking_send(ServerLogMsg::LogEnd { exit_code });
                        self.capturing = false;
                    }
                }
            }
        }
    }

    let writer_clone = writer.clone();
    let master_clone = master.clone();

    // Handle incoming WebSocket messages
    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            Message::Text(text) => {
                if let Ok(parsed) = serde_json::from_str::<ClientMsg>(&text) {
                    match parsed {
                        ClientMsg::Input { data } => {
                            if let Ok(mut w) = writer_clone.lock() {
                                let _ = w.write_all(data.as_bytes());
                                let _ = w.flush();
                            }
                            tracing::info!("Received input: {}", data);
                        }
                        ClientMsg::Run { data, id: _ } => {
                            if let Ok(mut w) = writer_clone.lock() {
                                // Just send the raw command. The shell integration (trap) will handle markers.
                                // We add a newline to ensure execution.
                                // For powershell and cmd.exe, we need \r\n
                                let cmd_str = if shell.ends_with("pwsh")
                                    || shell.ends_with("powershell")
                                    || is_cmd
                                {
                                    format!("{}\r\n", data)
                                } else {
                                    format!("{}\n", data)
                                };
                                let _ = w.write_all(cmd_str.as_bytes());
                                let _ = w.flush();
                            }
                            tracing::info!("Executed command: {}", data);
                        }
                        ClientMsg::Resize { cols, rows } => {
                            if let Ok(mut m) = master_clone.lock() {
                                let _ = m.resize(cols, rows);
                            }
                            tracing::info!("Resized PTY to {} cols and {} rows", cols, rows);
                        }
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    send_task.abort();
}
