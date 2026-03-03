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
    if let Ok(output) = std::process::Command::new("cmd").args(&["/c", "ver"]).output() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Windows 7 uses Version 6.1. Windows Vista is 6.0, Window 8 is 6.2/6.3.
        // We can treat any older Windows below 10 (Version 10.x) as needing WinPty.
        if !stdout.contains("Version 10.") && !stdout.contains("Version 11.") {
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
                MyCommand::WinPty(winpty_impl::WinPtyAdapter::new())
            } else {
                MyCommand::PortablePty(portable_pty_impl::PortablePtyAdapter::new())
            }
        }
    }

    pub fn spawn(&mut self, cmd: &str) -> anyhow::Result<(Reader, Writer)> {
        match self {
            MyCommand::PortablePty(adapter) => adapter.spawn(cmd),
            #[cfg(windows)]
            MyCommand::WinPty(adapter) => adapter.spawn(cmd),
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

        pub fn spawn(&mut self, cmd: &str) -> anyhow::Result<(Reader, Writer)> {
            let pty_system = NativePtySystem::default();
            let pair = pty_system.openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })?;
            
            let command = CommandBuilder::new(cmd);
            let child = pair.slave.spawn_command(command)?;
            
            let reader = pair.master.try_clone_reader()?;
            let writer = pair.master.take_writer()?;

            self.pair = Some(pair.master);
            self.child = Some(child);

            Ok((Box::new(reader), Box::new(writer)))
        }

        pub fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
            if let Some(master) = &self.pair {
                master.resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                }).map_err(Into::into)
            } else {
                Ok(())
            }
        }
    }
}

#[cfg(windows)]
pub mod winpty_impl {
    use super::{Reader, Writer};
    use std::io::{self, Read, Write};
    use std::sync::{Arc, Mutex};
    use winptyrs::{PTYArgs, PTYBackend, PTY};

    pub struct WinPtyAdapter {
        pty: Option<Arc<Mutex<PTY>>>,
    }

    impl WinPtyAdapter {
        pub fn new() -> Self {
            Self { pty: None }
        }

        pub fn spawn(&mut self, cmd: &str) -> anyhow::Result<(Reader, Writer)> {
            let mut args = PTYArgs::default();
            args.cols = 80;
            args.rows = 24;
            
            let mut pty = PTY::new_with_backend(&args, PTYBackend::WinPTY)
                .map_err(|e| anyhow::anyhow!("Failed to create winpty: {:?}", e))?;
                
            pty.spawn(std::ffi::OsString::from(cmd), None, None, None)
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
            let pty = self.0.lock().unwrap();
            match pty.read(true) {
                Ok(os_string) => {
                    let s = os_string.into_string().map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "Invalid UTF-8")
                    })?;
                    let bytes = s.as_bytes();
                    let len = std::cmp::min(buf.len(), bytes.len());
                    buf[..len].copy_from_slice(&bytes[..len]);
                    Ok(len)
                }
                Err(e) => Err(io::Error::new(io::ErrorKind::Other, format!("{:?}", e))),
            }
        }
    }

    struct WinPtyWriter(Arc<Mutex<PTY>>);
    
    // WinPTY 的 Write 需要转换
    impl Write for WinPtyWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let pty = self.0.lock().unwrap();
            let s = std::str::from_utf8(buf).map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, e)
            })?;
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
