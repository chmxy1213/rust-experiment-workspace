use anyhow::Result;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::fs::OpenOptions;
use std::io::{self, BufWriter, Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;

#[cfg(windows)]
use winptyrs::PTY;

#[cfg(windows)]
struct WinPtyReader {
    pty: Arc<Mutex<PTY>>,
}

#[cfg(windows)]
impl Read for WinPtyReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Ok(pty) = self.pty.lock() {
            let output = pty.read(true).unwrap();
            let bytes = output.as_encoded_bytes();
            let len = bytes.len().min(buf.len());
            buf[..len].copy_from_slice(&bytes[..len]);
            Ok(len)
        } else {
            Err(io::Error::new(io::ErrorKind::Other, "Failed to lock PTY"))
        }
    }
}

#[cfg(windows)]
struct WinPtyWriter {
    pty: Arc<Mutex<PTY>>,
}

#[cfg(windows)]
impl Write for WinPtyWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Ok(pty) = self.pty.lock() {
            let size = pty
                .write(String::from_utf8(buf.to_vec()).unwrap().into())
                .unwrap();
            Ok(size as usize)
        } else {
            Err(io::Error::new(io::ErrorKind::Other, "Failed to lock PTY"))
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        // WinPTY 可能不需要显式 flush
        Ok(())
    }
}

#[cfg(windows)]
fn is_windows_10_or_higher() -> bool {
    // 使用环境变量检测 Windows 版本
    // Windows 10 的版本号是 10.0
    if let Ok(version) = std::env::var("OS") {
        if version.contains("Windows") {
            // 尝试读取版本信息，如果失败则默认使用 ConPTY (假设是新版本)
            return std::env::var("PROCESSOR_ARCHITECTURE").is_ok();
        }
    }

    // 另一种方法：检查 Windows 构建号
    // Windows 10 build >= 17763 支持 ConPTY
    // 简化处理：默认使用 ConPTY，除非明确设置环境变量
    std::env::var("USE_WINPTY").is_err()
}

enum PtyBackend {
    Portable(Box<dyn portable_pty::MasterPty + Send>),
    #[cfg(windows)]
    WinPty(winptyrs::PTY),
}

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

    let cwd = std::env::current_dir()?;

    #[cfg(windows)]
    let use_winpty = !is_windows_10_or_higher();

    #[cfg(windows)]
    let script_path = cwd.join("powershell_recorder.ps1");

    #[cfg(not(windows))]
    let script_path = cwd.join("bash_recorder.sh");

    // 根据平台和版本选择不同的 PTY 实现
    #[cfg(windows)]
    let (mut reader, mut writer, _child) = if use_winpty {
        // Windows 7/8: 使用 WinPTY
        eprintln!("Using WinPTY backend (Windows 7/8 detected)");

        use winptyrs::*;

        let mut pty = PTY::new(&PTYArgs {
            cols: 80,
            rows: 24,
            agent_config: AgentConfig::WINPTY_FLAG_COLOR_ESCAPES,
            ..Default::default()
        })
        .unwrap();

        let cmd = format!(
            "powershell.exe -NoExit -NoLogo -ExecutionPolicy Bypass -File \"{}\"",
            script_path.display()
        );

        pty.spawn(cmd.into(), None, None, None).unwrap();

        let pty = Arc::new(Mutex::new(pty));

        // 先创建 reader 和 writer
        let reader = WinPtyReader {
            pty: Arc::clone(&pty),
        };
        let writer = WinPtyWriter {
            pty: Arc::clone(&pty),
        };

        (
            Box::new(reader) as Box<dyn Read + Send>,
            Box::new(writer) as Box<dyn Write + Send>,
            None,
        )
    } else {
        // Windows 10+: 使用 ConPTY
        eprintln!("Using ConPTY backend (Windows 10+ detected)");

        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new("powershell.exe");
        cmd.arg("-NoExit");
        cmd.arg("-NoLogo");
        cmd.arg("-ExecutionPolicy");
        cmd.arg("Bypass");
        cmd.arg("-File");
        cmd.arg(script_path);

        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        (
            Box::new(reader) as Box<dyn Read + Send>,
            Box::new(writer) as Box<dyn Write + Send>,
            Some(child),
        )
    };

    #[cfg(not(windows))]
    let (mut reader, mut writer, _child) = {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new("bash");
        cmd.arg("--rcfile");
        cmd.arg(script_path);

        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        (
            Box::new(reader) as Box<dyn Read + Send>,
            Box::new(writer) as Box<dyn Write + Send>,
            child,
        )
    };

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

    Ok(())
}
