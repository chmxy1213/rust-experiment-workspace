use anyhow::Result;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::fs::OpenOptions;
use std::io::{self, Read, Write, BufWriter};
use std::sync::{Arc, Mutex};
use std::thread;

struct CommandSession {
    command: String,
    start_time: std::time::SystemTime,
    output: Vec<u8>,
}

struct LogInterpreter {
    log_file: Arc<Mutex<BufWriter<std::fs::File>>>,
    current_session: Option<CommandSession>,
}

impl LogInterpreter {
    fn new(log_file: Arc<Mutex<BufWriter<std::fs::File>>>) -> Self {
        Self { 
            log_file,
            current_session: None,
        }
    }

    fn capture_output(&mut self, data: &[u8]) {
        if let Some(session) = &mut self.current_session {
            session.output.extend_from_slice(data);
        }
    }
}

impl vte::Perform for LogInterpreter {
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.is_empty() {
            return;
        }

        if params[0] == b"666" {
            if params.len() < 2 {
                return;
            }

            let type_str = String::from_utf8_lossy(params[1]);

            match type_str.as_ref() {
                "CMD_START" => {
                    // 命令开始执行
                    if params.len() >= 3 {
                        let command = String::from_utf8_lossy(params[2]).to_string();
                        
                        if let Ok(mut log) = self.log_file.lock() {
                            let _ = writeln!(log, "\n=== Command Started ===");
                            let _ = writeln!(log, "Command: {}", command);
                            let _ = writeln!(log, "Time: {:?}", std::time::SystemTime::now());
                            let _ = log.flush();
                        }

                        self.current_session = Some(CommandSession {
                            command,
                            start_time: std::time::SystemTime::now(),
                            output: Vec::new(),
                        });
                    }
                }
                "CMD_END" => {
                    // 命令执行完成
                    if let Some(session) = self.current_session.take() {
                        let exit_code = if params.len() >= 3 {
                            String::from_utf8_lossy(params[2]).to_string()
                        } else {
                            "unknown".to_string()
                        };

                        if let Ok(mut log) = self.log_file.lock() {
                            let duration = std::time::SystemTime::now()
                                .duration_since(session.start_time)
                                .unwrap_or_default();
                            
                            let _ = writeln!(log, "--- Output ---");
                            let output_str = String::from_utf8_lossy(&session.output);
                            let _ = write!(log, "{}", output_str);
                            let _ = writeln!(log, "\n--- End Output ---");
                            let _ = writeln!(log, "Exit Code: {}", exit_code);
                            let _ = writeln!(log, "Duration: {:?}", duration);
                            let _ = writeln!(log, "=== Command Ended ===\n");
                            let _ = log.flush();
                        }
                    }
                }
                "PWD" => {
                    // 可选：记录工作目录变化
                    if params.len() >= 3 {
                        let pwd = String::from_utf8_lossy(params[2]);
                        if let Ok(mut log) = self.log_file.lock() {
                            let _ = writeln!(log, "[PWD] {}", pwd);
                            let _ = log.flush();
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn main() -> Result<()> {
    // 创建命令日志文件
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open("shell_commands.log")?;
    let log_file = Arc::new(Mutex::new(BufWriter::new(log_file)));

    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let cwd = std::env::current_dir()?;
    
    // 检测操作系统，选择合适的 shell
    let mut cmd = if cfg!(windows) {
        let script_path = cwd.join("powershell_recorder.ps1");
        let mut c = CommandBuilder::new("powershell.exe");
        c.arg("-NoExit");
        c.arg("-NoLogo");
        c.arg("-ExecutionPolicy");
        c.arg("Bypass");
        c.arg("-File");
        c.arg(script_path);
        c
    } else {
        let script_path = cwd.join("bash_recorder.sh");
        let mut c = CommandBuilder::new("bash");
        c.arg("--rcfile");
        c.arg(script_path);
        c
    };

    let mut child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;

    enable_raw_mode()?;

    thread::spawn(move || {
        let mut stdin = io::stdin();
        let _ = io::copy(&mut stdin, &mut writer);
    });

    let mut parser = vte::Parser::new();
    let mut interpreter = LogInterpreter::new(log_file);
    let mut stdout = io::stdout();
    let mut buf = [0u8; 4096];

    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let data = &buf[..n];
                
                // 输出到控制台
                stdout.write_all(data).unwrap_or(());
                stdout.flush().unwrap_or(());

                // 捕获命令输出（去除 ANSI 控制序列的原始数据）
                interpreter.capture_output(data);

                // 解析 OSC 序列
                for byte in data {
                    parser.advance(&mut interpreter, *byte);
                }
            }
            Err(_) => break,
        }
    }

    disable_raw_mode()?;
    println!("Session ended.");
    let _ = child.wait();

    Ok(())
}
