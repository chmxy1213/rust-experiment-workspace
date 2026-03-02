use std::io::{Read, Write};

use portable_pty::{NativePtySystem, PtySize, PtySystem};

pub type Reader = Box<dyn Read + Send>;
pub type Writer = Box<dyn Write + Send>;

pub enum MyCommand {
    PortablePty,
    WinPty,
}

impl MyCommand {
    fn spawn(&mut self, cmd: &str) -> anyhow::Result<()> {
        match self {
            MyCommand::PortablePty => {
                let pty_system = NativePtySystem::default();

                let pair = pty_system
                    .openpty(PtySize {
                        rows: 24,
                        cols: 80,
                        pixel_width: 0,
                        pixel_height: 0,
                    })
                    .expect("Failed to create PTY");

                todo!()
            }
            MyCommand::WinPty => {
                todo!()
            }
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        todo!()
    }
}
