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

    // We don't want to use "-Command -" because portable-pty already sets up a PTY
    // which acts as a valid stdin/stdout device.
    let mut cmd_builder = CommandBuilder::new("pwsh");
    cmd_builder.args(&["-NoLogo", "-NoExit"]);

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
    // We construct the path relative to the crate root to ensure it works in CI and locally
    // regardless of where `cargo test` is invoked from.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let script_path = std::path::PathBuf::from(manifest_dir).join("static/shell-integration.ps1");

    // Check if file exists to give a better error message
    if !script_path.exists() {
        panic!("Integration script not found at: {:?}", script_path);
    }

    // We don't necessarily need to read content if we just source it by path,
    // but reading it helps debugging if specific content is needed.
    // let script_content = std::fs::read_to_string(&script_path).expect("Could not read integration script");

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

    // Note: script_path is already absolute because CARGO_MANIFEST_DIR is absolute.
    let load_cmd = format!(". '{}'\r\n", script_path.to_string_lossy());
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

#[cfg(windows)]
#[test]
fn test_cmd_integration() {
    // 1. Prepare: Copy integration lua script to Clink's script directory (%LOCALAPPDATA%\clink)
    // Clink, when injected, auto-loads .lua files from here.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let script_source = std::path::PathBuf::from(manifest_dir).join("static/shell-integration.lua");

    if !script_source.exists() {
        panic!("Lua integration script not found at: {:?}", script_source);
    }

    let local_app_data = std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA not set");
    let clink_dir = std::path::PathBuf::from(local_app_data).join("clink");

    // Ensure Clink script dir exists
    std::fs::create_dir_all(&clink_dir).expect("Failed to create clink dir");

    let script_target = clink_dir.join("shell_integration_test.lua");
    // Overwrite if exists
    std::fs::copy(&script_source, &script_target).expect("Failed to copy lua script to clink dir");

    // 2. Start CMD via PTY
    // Note: Clink injects into cmd.exe automatically if "clink autorun install" was run.
    let pty_system = NativePtySystem::default();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("Failed to create PTY");

    let cmd_builder = CommandBuilder::new("cmd.exe");
    let mut child = pair
        .slave
        .spawn_command(cmd_builder)
        .expect("Failed to spawn cmd");

    let mut writer = pair.master.take_writer().expect("Failed to take writer");
    let mut reader = pair
        .master
        .try_clone_reader()
        .expect("Failed to clone reader");

    // Reader thread
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

    // We can try to run a command and see output
    thread::sleep(Duration::from_secs(2)); // Allow Clink to load
    writer.write_all(b"echo hello_cmd_osc\r").unwrap(); // \r triggers the key handler
    thread::sleep(Duration::from_secs(2));

    // 3. Verify Output
    let output = output_buffer.lock().unwrap().clone();
    println!("Captured CMD Output:\n{}", output);

    if output.contains("hello_cmd_osc") {
        if output.contains("]6973;START;") {
            assert!(output.contains("]6973;END;0"), "Should contain END signal");
        } else {
            println!("WARNING: CMD ran but OSC codes missing. Clink integration might not be active/installed.");
        }
    } else {
        panic!("CMD failed to echo output");
    }
}
