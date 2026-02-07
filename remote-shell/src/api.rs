//! Web API

use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
    thread,
};

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::{Html, IntoResponse},
};
use futures::{sink::SinkExt, stream::StreamExt};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use regex::Regex;
use tokio::sync::mpsc;

use crate::{ClientMsg, ServerLogMsg};

pub async fn index_handler() -> Html<&'static str> {
    // Force recompilation when index.html changes by including bytes, though include_str matches too.
    // We add a cache buster comment implicitly by changing main.rs
    Html(include_str!("../static/index.html"))
}

pub async fn ws_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(socket: WebSocket) {
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

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string());
    let is_bash = shell.ends_with("bash");
    let is_zsh = shell.ends_with("zsh");

    let mut cmd = CommandBuilder::new(&shell);

    if is_bash {
        cmd.args(&["--rcfile", "static/shell-integration.bash"]);
    }

    cmd.cwd(std::env::current_dir().unwrap());
    cmd.env("TERM", "xterm-256color");

    let _child = pair
        .slave
        .spawn_command(cmd)
        .expect("Failed to spawn shell");

    let master = pair.master;
    let mut reader = master.try_clone_reader().expect("Failed to clone reader");
    let writer = master.take_writer().expect("Failed to take writer");

    // We wrap writer in a Mutex to use it in the loop (which is technically blocking, but fast for buffer write)
    // Using Arc<Mutex<...>> for thread safety if we were to share it, here we clone for the loop.
    let writer = Arc::new(Mutex::new(writer));
    let master = Arc::new(Mutex::new(master));

    // Initialize Shell Integration for Zsh (since we can't use --rcfile)
    if is_zsh {
        if let Ok(mut w) = writer.lock() {
            // Source the integration script
            // We add a newline to ensure it executes
            // To hide the command itself from history/view, usually we can't easily do it via injection
            // without "space" prefix (if configured) or just accept it prints once.
            let init_cmd = "source static/shell-integration.zsh\n";
            let _ = w.write_all(init_cmd.as_bytes());
            let _ = w.flush();
        }
    }

    let (tx_output, mut rx_output) = mpsc::channel::<Vec<u8>>(32);
    let (tx_log, mut rx_log) = mpsc::channel::<ServerLogMsg>(32);

    // Spawn blocking thread for reading PTY
    thread::spawn(move || {
        let mut buf = [0u8; 1024];
        let mut parsing_str = String::new();
        let mut is_capturing = false;

        // Use normal strings to safely handle control characters (\x1b, \x07)
        let start_re = Regex::new("\x1b]6973;START\x07").expect("Invalid START regex");
        let end_re = Regex::new("\x1b]6973;END;(\\d+)\x07").expect("Invalid END regex");

        // Regex to strip ANSI CSI (\x1b[ ... char) and OSC (\x1b] ... \x07)
        // We use string literals so \x1b and \x07 are actual bytes.
        // Double backslashes needed for regex metacharacters like \[ and \d.
        let ansi_re = Regex::new("(\\x1b\\[[0-9;?]*[a-zA-Z])|(\\x1b][^\\x07]*\\x07)")
            .expect("Invalid ANSI regex");

        loop {
            match reader.read(&mut buf) {
                Ok(n) if n > 0 => {
                    let data = buf[..n].to_vec();
                    // Send RAW output to frontend terminal
                    if tx_output.blocking_send(data.clone()).is_err() {
                        break;
                    }

                    // --- Log Extraction Logic ---
                    // Convert to string (lossy is fine for logs)
                    let s = String::from_utf8_lossy(&data);
                    parsing_str.push_str(&s);

                    loop {
                        if !is_capturing {
                            if let Some(mat) = start_re.find(&parsing_str) {
                                // Found START. Discard everything before (and including) START
                                parsing_str = parsing_str[mat.end()..].to_string();
                                is_capturing = true;
                                // Loop again to see if END is also present immediately
                                continue;
                            } else {
                                // No START found. Keep tail part just in case START is split.
                                // Max length of START marker is ~15 chars.
                                if parsing_str.len() > 20 {
                                    parsing_str = parsing_str[parsing_str.len() - 20..].to_string();
                                }
                                break;
                            }
                        } else {
                            // We are capturing. Look for END.
                            if let Some(mat) = end_re.find(&parsing_str) {
                                // Found END. Extract content.
                                let content_raw = &parsing_str[..mat.start()];
                                let captures = end_re.captures(&parsing_str).unwrap();
                                let exit_code_str = captures.get(1).map_or("0", |m| m.as_str());
                                let exit_code = exit_code_str.parse::<i32>().unwrap_or(0);

                                // Clean content
                                let clean_content =
                                    ansi_re.replace_all(content_raw, "").to_string();

                                // Send accumulated content
                                if !clean_content.is_empty() {
                                    let _ = tx_log.blocking_send(ServerLogMsg::LogOutput {
                                        data: clean_content,
                                    });
                                }
                                // Send END
                                let _ = tx_log.blocking_send(ServerLogMsg::LogEnd { exit_code });

                                // Remove everything up to END match
                                parsing_str = parsing_str[mat.end()..].to_string();
                                is_capturing = false;
                                continue;
                            } else {
                                // No END yet.
                                // We can safely send everything except the last few chars (in case END is split).
                                // Max END marker len is ~20 chars ("\x1b]...END;123\x07")
                                let reserve = 30;
                                if parsing_str.len() > reserve {
                                    let split_idx = parsing_str.len() - reserve;
                                    let content_raw = &parsing_str[..split_idx];
                                    let clean_content =
                                        ansi_re.replace_all(content_raw, "").to_string();

                                    if !clean_content.is_empty() {
                                        let _ = tx_log.blocking_send(ServerLogMsg::LogOutput {
                                            data: clean_content,
                                        });
                                    }

                                    parsing_str = parsing_str[split_idx..].to_string();
                                }
                                break;
                            }
                        }
                    }
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
                                let cmd_str = format!("{}\n", data);
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
