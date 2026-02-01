use anyhow::Result;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{self, Read, Write};
use std::thread;

struct LogInterpreter;

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

            let mut payload = String::new();
            for (i, p) in params.iter().enumerate().skip(2) {
                if i > 2 {
                    payload.push(';');
                }
                payload.push_str(&String::from_utf8_lossy(p));
            }

            eprintln!(
                "\r\n\x1b[32m[RECORDER] Event: {} | Payload: {}\x1b[0m\r",
                type_str, payload
            );
        }
    }
}

fn main() -> Result<()> {
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let cwd = std::env::current_dir()?;
    let script_path = cwd.join("bash_recorder.sh");

    let mut cmd = CommandBuilder::new("bash");
    cmd.arg("--rcfile");
    cmd.arg(script_path);

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
    let mut interpreter = LogInterpreter;
    let mut stdout = io::stdout();
    let mut buf = [0u8; 4096];

    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let data = &buf[..n];
                stdout.write_all(data).unwrap_or(());
                stdout.flush().unwrap_or(());

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
