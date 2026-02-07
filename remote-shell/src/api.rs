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
                    
                    // Flush any pending text after processing a chunk (optional but good for responsiveness)

                    // interpreter.flush(); // Actually let's not flush too eagerly to avoid tiny chunks, 
                    // or maybe flushing on newlines is better. 
                    // But our flush logic is checking buffer emptiness. 
                    // Let's do a flush if we have significant data or just trust the next chunk.
                    // Implementation choice: flush every chunk processing for real-time logs?
                    // Yes, logs-container updates are better in real-time.
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
             // 1. 6973;START
             // 2. 6973;END;0
            if params.len() > 1 {
                let cmd = params[1];
                
                if cmd == b"START" {
                    self.capturing = true;
                    self.buffer.clear(); 
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
