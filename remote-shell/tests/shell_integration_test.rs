#[cfg(windows)]
use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

#[cfg(windows)]
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

// Only run this test on Windows because it tests Windows-specific shells (cmd, powershell)
// or assumes their availability.
#[cfg(windows)]
#[test]
fn test_pwsh_integration() {
    let pty_system = NativePtySystem::default();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("Failed to create PTY");

    let mut cmd_builder = CommandBuilder::new("pwsh");
    cmd_builder.args(&["-NoLogo", "-NoExit", "-Command", "-"]); // interactive mode reading from stdin

    let mut child = pair
        .slave
        .spawn_command(cmd_builder)
        .expect("Failed to spawn pwsh");

    let mut writer = pair.master.take_writer().expect("Failed to take writer");
    let mut reader = pair
        .master
        .try_clone_reader()
        .expect("Failed to clone reader");

    // Spawn a thread to read output
    let output_buffer = Arc::new(Mutex::new(String::new()));
    let output_buffer_clone = output_buffer.clone();

    thread::spawn(move || {
        let mut buf = [0u8; 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(n) if n > 0 => {
                    let s = String::from_utf8_lossy(&buf[0..n]);
                    let mut locked = output_buffer_clone.lock().unwrap();
                    locked.push_str(&s);
                }
                _ => break,
            }
        }
    });

    // 1. Inject the integration script
    // We read the actual file content to simulate exactly what the app does,
    // or we can just send the content if we trust the file is there.
    // In a test, better to rely on what's committed. context: the test runs from root usually.
    let script_path = "static/shell-integration.ps1";
    let script_content =
        std::fs::read_to_string(script_path).expect("Could not read integration script");

    // Send script to PTY
    // We wrap it in a block or invoke-expression to ensure it runs
    // But since it defines functions, we just paste it.
    // However, pasting large text might hit buffer limits.
    // Better strategy for test: Write it to a temp file and dot-source it?
    // Or just write it chunk by chunk.

    // For simplicity in this test, let's assume we can just write it.
    // But wait, the standard approach in the app is:
    // let init_cmd = ". ./static/shell-integration.ps1\n";
    // We should try to use the absolute path of the test runtime.

    let abs_path = std::fs::canonicalize(script_path).expect("Failed to canonicalize path");
    let load_cmd = format!(". '{}'\r\n", abs_path.to_string_lossy());
    writer.write_all(load_cmd.as_bytes()).unwrap();

    // Give it time to load
    thread::sleep(Duration::from_secs(2));

    // 2. Run a command
    // We expect OSC 6973 sequences.
    // Pwsh integration hooks Enter key via PSReadLine.
    // BUT: PSReadLine doesn't work well in a non-interactive PTY harness without valid input device simulation sometimes.
    // However, providing input via PTY master writer usually works for PSReadLine.

    writer.write_all(b"echo 'Hello World'\r").unwrap(); // \r triggers the key handler

    thread::sleep(Duration::from_secs(2));

    // 3. Verify Output
    let output = output_buffer.lock().unwrap().clone();

    println!("Captured Output:\n{}", output);

    // Check for START signal
    // Sequence: OSC 6973 ; START ; ...
    assert!(
        output.contains("\x1b]6973;START;"),
        "Output should contain START signal"
    );
    assert!(
        output.contains("\x1b]6973;END;0"),
        "Output should contain END signal with success code"
    );
}
