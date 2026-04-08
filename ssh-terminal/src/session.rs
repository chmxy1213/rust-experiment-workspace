use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

// 本地 shell 会话：通过 PTY 与 shell 交互，使用读写线程与 UI 解耦。
pub struct LocalShellSession {
    // UI -> writer 线程
    input_tx: Sender<Vec<u8>>,
    // reader 线程 -> UI
    output_rx: Receiver<Vec<u8>>,
    // resize 需要直接访问 master
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    // shell 退出后标记会话结束，避免 UI 继续写入报错。
    closed: Arc<AtomicBool>,
    _child: Box<dyn portable_pty::Child + Send + Sync>,
    _reader_thread: thread::JoinHandle<()>,
    _writer_thread: thread::JoinHandle<()>,
}

impl LocalShellSession {
    pub fn spawn_default(cols: u16, rows: u16) -> Result<Self> {
        // 创建 PTY 并启动用户默认 shell。
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to open PTY")?;

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        let mut command = CommandBuilder::new(shell);
        // 保持终端能力与 ANSI 行为一致。
        command.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(command)
            .context("failed to spawn shell")?;
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .context("failed to clone PTY reader")?;
        let writer = pair
            .master
            .take_writer()
            .context("failed to get PTY writer")?;

        let master = Arc::new(Mutex::new(pair.master));
        let closed = Arc::new(AtomicBool::new(false));
        let (input_tx, input_rx) = mpsc::channel::<Vec<u8>>();
        let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>();

        let reader_thread = spawn_reader_thread(reader, output_tx, closed.clone());
        let writer_thread = spawn_writer_thread(writer, input_rx, closed.clone());

        Ok(Self {
            input_tx,
            output_rx,
            master,
            closed,
            _child: child,
            _reader_thread: reader_thread,
            _writer_thread: writer_thread,
        })
    }

    pub fn send_input(&self, bytes: &[u8]) -> Result<()> {
        if self.is_closed() {
            return Ok(());
        }

        // 通过通道异步发送，避免 UI 线程阻塞在 I/O。
        self.input_tx
            .send(bytes.to_vec())
            .context("failed to send input to PTY")
    }

    pub fn try_read(&self) -> Option<Vec<u8>> {
        // 非阻塞轮询，配合渲染循环高频刷新。
        self.output_rx.try_recv().ok()
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        if self.is_closed() {
            return Ok(());
        }

        // 窗口尺寸变化时把新的字符网格同步到 shell。
        let master = self.master.lock().expect("PTY master mutex poisoned");
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to resize PTY")
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }
}

fn spawn_reader_thread(
    mut reader: Box<dyn Read + Send>,
    output_tx: Sender<Vec<u8>>,
    closed: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        // 分块读取 PTY 输出并推送给 UI。
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    closed.store(true, Ordering::Relaxed);
                    break;
                }
                Ok(size) => {
                    if output_tx.send(buffer[..size].to_vec()).is_err() {
                        closed.store(true, Ordering::Relaxed);
                        break;
                    }
                }
                Err(_) => {
                    closed.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }
    })
}

fn spawn_writer_thread(
    mut writer: Box<dyn Write + Send>,
    input_rx: Receiver<Vec<u8>>,
    closed: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        // 逐条写入并 flush，降低交互输入延迟。
        while let Ok(bytes) = input_rx.recv() {
            if writer.write_all(&bytes).is_err() {
                closed.store(true, Ordering::Relaxed);
                break;
            }
            let _ = writer.flush();
        }
    })
}
