use std::io::{Read, Write};

pub type Reader = Box<dyn Read + Send>;
pub type Writer = Box<dyn Write + Send>;

#[cfg(not(windows))]
pub enum MyCommand {
    PortablePty(portable_pty_impl::PortablePtyAdapter),
}

#[cfg(windows)]
pub enum MyCommand {
    PortablePty(portable_pty_impl::PortablePtyAdapter),
    WinPty(winpty_impl::WinPtyAdapter),
}

#[cfg(windows)]
fn is_windows_7() -> bool {
    if let Ok(output) = std::process::Command::new("cmd")
        .args(&["/c", "ver"])
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        tracing::debug!("Windows version output: {}", stdout);
        // Windows 7 uses Version 6.1. Windows Vista is 6.0, Window 8 is 6.2/6.3.
        // We can treat any older Windows below 10 (Version 10.x) as needing WinPty.
        // Windows 11 returns "Version 10.0..." just like Windows 10
        if !stdout.contains("10.") {
            return true;
        }
    }
    // Default fallback to true for safety if we fail to detect purely?
    // Actually, portable-pty will panic on Win 7 if we guess wrong, but ConPTY is for Win10+.
    false
}

impl MyCommand {
    pub fn new() -> Self {
        #[cfg(not(windows))]
        {
            MyCommand::PortablePty(portable_pty_impl::PortablePtyAdapter::new())
        }
        #[cfg(windows)]
        {
            if is_windows_7() {
                tracing::debug!("Detected Windows 7, using WinPty");
                MyCommand::WinPty(winpty_impl::WinPtyAdapter::new())
            } else {
                tracing::debug!("Detected Windows 10 or later, using PortablePty");
                MyCommand::PortablePty(portable_pty_impl::PortablePtyAdapter::new())
            }
        }
    }

    pub fn spawn<I, S>(&mut self, cmd: &str, args: I) -> anyhow::Result<(Reader, Writer)>
    where
        I: IntoIterator<Item = S> + Clone,
        S: AsRef<std::ffi::OsStr>,
    {
        match self {
            MyCommand::PortablePty(adapter) => adapter.spawn(cmd, args),
            #[cfg(windows)]
            MyCommand::WinPty(adapter) => adapter.spawn(cmd, args),
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        match self {
            MyCommand::PortablePty(adapter) => adapter.resize(cols, rows),
            #[cfg(windows)]
            MyCommand::WinPty(adapter) => adapter.resize(cols, rows),
        }
    }
}

pub mod portable_pty_impl {
    use super::{Reader, Writer};
    use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

    pub struct PortablePtyAdapter {
        pair: Option<Box<dyn portable_pty::MasterPty + Send>>,
        child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    }

    impl PortablePtyAdapter {
        pub fn new() -> Self {
            Self {
                pair: None,
                child: None,
            }
        }

        pub fn spawn<I, S>(&mut self, cmd: &str, args: I) -> anyhow::Result<(Reader, Writer)>
        where
            I: IntoIterator<Item = S>,
            S: AsRef<std::ffi::OsStr>,
        {
            let pty_system = NativePtySystem::default();
            let pair = pty_system.openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })?;

            let mut command = CommandBuilder::new(cmd);
            command.args(args);
            let child = pair.slave.spawn_command(command)?;

            let reader = pair.master.try_clone_reader()?;
            let writer = pair.master.take_writer()?;

            self.pair = Some(pair.master);
            self.child = Some(child);

            Ok((Box::new(reader), Box::new(writer)))
        }

        pub fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
            if let Some(master) = &self.pair {
                master
                    .resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    })
                    .map_err(Into::into)
            } else {
                Ok(())
            }
        }
    }
}

#[cfg(windows)]
pub mod winpty_impl {
    use super::{Reader, Writer};
    use std::ffi::{OsStr, OsString};
    use std::io::{self, Read, Write};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use winptyrs::{PTYArgs, PTYBackend, PTY};

    fn quote_windows_arg(arg: &OsStr) -> String {
        let text = arg.to_string_lossy();
        if text.is_empty() {
            return "\"\"".to_string();
        }

        let needs_quotes = text.chars().any(|c| c == ' ' || c == '\t' || c == '"');
        if !needs_quotes {
            return text.into_owned();
        }

        let escaped = text.replace('"', "\\\"");
        format!("\"{}\"", escaped)
    }

    fn resolve_windows_shell_path(cmd: &str) -> OsString {
        let cmd_lower = cmd.to_ascii_lowercase();
        let path = Path::new(cmd);
        if path.components().count() > 1 {
            return OsString::from(cmd);
        }

        let system_root =
            std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from("C:\\Windows"));
        let system_root = PathBuf::from(system_root);

        match cmd_lower.as_str() {
            "cmd" | "cmd.exe" => system_root
                .join("System32")
                .join("cmd.exe")
                .into_os_string(),
            "powershell" | "powershell.exe" => system_root
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe")
                .into_os_string(),
            _ => OsString::from(cmd),
        }
    }

    pub struct WinPtyAdapter {
        pty: Option<Arc<Mutex<PTY>>>,
    }

    impl WinPtyAdapter {
        pub fn new() -> Self {
            Self { pty: None }
        }

        pub fn spawn<I, S>(&mut self, cmd: &str, args: I) -> anyhow::Result<(Reader, Writer)>
        where
            I: IntoIterator<Item = S>,
            S: AsRef<std::ffi::OsStr>,
        {
            let app = resolve_windows_shell_path(cmd);
            let arg_values: Vec<OsString> = args
                .into_iter()
                .map(|arg| arg.as_ref().to_os_string())
                .collect();
            let cmdline = if arg_values.is_empty() {
                None
            } else {
                Some(OsString::from(
                    arg_values
                        .iter()
                        .map(|arg| quote_windows_arg(arg.as_os_str()))
                        .collect::<Vec<_>>()
                        .join(" "),
                ))
            };

            let mut args_pty = PTYArgs::default();
            args_pty.cols = 80;
            args_pty.rows = 24;

            let mut pty = PTY::new_with_backend(&args_pty, PTYBackend::WinPTY)
                .map_err(|e| anyhow::anyhow!("Failed to create winpty: {:?}", e))?;

            tracing::info!(
                "Spawning WinPTY process: app={} cmdline={:?}",
                PathBuf::from(&app).display(),
                cmdline.as_ref().map(|s| s.to_string_lossy().to_string())
            );

            pty.spawn(app, cmdline, None, None)
                .map_err(|e| anyhow::anyhow!("Failed to spawn winpty: {:?}", e))?;

            let pty_arc = Arc::new(Mutex::new(pty));
            self.pty = Some(pty_arc.clone());

            Ok((
                Box::new(WinPtyReader(pty_arc.clone())),
                Box::new(WinPtyWriter(pty_arc)),
            ))
        }

        pub fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
            if let Some(pty) = &self.pty {
                let pty = pty.lock().unwrap();
                pty.set_size(cols as i32, rows as i32)
                    .map_err(|e| anyhow::anyhow!("Failed to resize winpty: {:?}", e))?;
            }
            Ok(())
        }
    }

    struct WinPtyReader(Arc<Mutex<PTY>>);

    // WinPTY 要求实现 std::io::Read，因为 winpty-rs 的 read(&self, blocking) 直接返回 OsString。
    impl Read for WinPtyReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            loop {
                let read_result = {
                    let pty = self.0.lock().unwrap();
                    pty.read(false)
                };

                match read_result {
                    Ok(os_string) => {
                        let s = os_string.into_string().map_err(|_| {
                            io::Error::new(io::ErrorKind::InvalidData, "Invalid UTF-8")
                        })?;

                        if s.is_empty() {
                            std::thread::sleep(Duration::from_millis(10));
                            continue;
                        }

                        let bytes = s.as_bytes();
                        let len = std::cmp::min(buf.len(), bytes.len());
                        buf[..len].copy_from_slice(&bytes[..len]);
                        return Ok(len);
                    }
                    Err(e) => return Err(io::Error::new(io::ErrorKind::Other, format!("{:?}", e))),
                }
            }
        }
    }

    struct WinPtyWriter(Arc<Mutex<PTY>>);

    // WinPTY 的 Write 需要转换
    impl Write for WinPtyWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let pty = self.0.lock().unwrap();
            let s = std::str::from_utf8(buf)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            match pty.write(std::ffi::OsString::from(s)) {
                Ok(written) => Ok(written as usize),
                Err(e) => Err(io::Error::new(io::ErrorKind::Other, format!("{:?}", e))),
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
