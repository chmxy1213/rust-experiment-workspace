//! Web API

use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
    thread,
};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query,
    },
    response::{Html, IntoResponse},
};
use futures::{sink::SinkExt, stream::StreamExt};
use portable_pty::{NativePtySystem, PtySize, PtySystem};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::static_files::Asset;
use crate::{ClientMsg, ServerLogMsg};

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
    let pty_system = NativePtySystem::default();

    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("Failed to create PTY");

    let default_shell = std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string());
    let shell = params.shell.unwrap_or(default_shell);
    let is_bash = shell.ends_with("bash");
    let is_zsh = shell.ends_with("zsh");
    let is_pwsh = shell.ends_with("pwsh") || shell.ends_with("powershell");

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

    let mut cmd_builder = portable_pty::CommandBuilder::new(&shell);

    if is_bash {
        if let Some((_, ref path_str)) = temp_script_path {
            cmd_builder.args(&["--rcfile", path_str]);
        }
    }
    // For pwsh, we often need -NoExit if we pass a file, but here we just spawn shell
    // and rely on injection via stdin like zsh to avoid path complexity
    if is_pwsh {
        cmd_builder.args(&["-NoLogo", "-NoExit"]);
    }

    cmd_builder.cwd(std::env::current_dir().unwrap());
    cmd_builder.env("TERM", "xterm-256color");

    let mut child_result = pair.slave.spawn_command(cmd_builder);

    // Fallback for pwsh -> powershell on Windows
    if child_result.is_err() && shell == "pwsh" && cfg!(target_os = "windows") {
        tracing::warn!("Failed to spawn pwsh, falling back to powershell");
        let mut fallback_cmd = portable_pty::CommandBuilder::new("powershell");
        fallback_cmd.args(&["-NoLogo", "-NoExit"]);
        fallback_cmd.cwd(std::env::current_dir().unwrap());
        fallback_cmd.env("TERM", "xterm-256color");
        child_result = pair.slave.spawn_command(fallback_cmd);
    }

    let _child = match child_result {
        Ok(child) => child,
        Err(e) => {
            tracing::error!("Failed to spawn shell: {}", e);
            return;
        }
    };

    let master = pair.master;
    let mut reader = master.try_clone_reader().expect("Failed to clone reader");
    let writer = master.take_writer().expect("Failed to take writer");

    let writer = Arc::new(Mutex::new(writer));
    let master = Arc::new(Mutex::new(master));

    // Inject scripts for shells that don't support --rcfile easily or where we prefer dynamic loading
    if is_zsh {
        if let Ok(mut w) = writer.lock() {
            if let Some((_, ref path_str)) = temp_script_path {
                let init_cmd = format!("source {}\n", path_str);
                let _ = w.write_all(init_cmd.as_bytes());
                let _ = w.flush();
            }
        }
    } else if is_pwsh {
        if let Ok(mut w) = writer.lock() {
            // PowerShell sourcing
            if let Some((_, ref path_str)) = temp_script_path {
                let init_cmd = format!(". {}\n", path_str);
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
                                // For powershell, we might need \r\n
                                let cmd_str = if shell.ends_with("pwsh") || shell.ends_with("powershell") {
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
                            if let Ok(m) = master_clone.lock() {
                                let _ = m.resize(PtySize {
                                    rows,
                                    cols,
                                    pixel_width: 0,
                                    pixel_height: 0,
                                });
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
