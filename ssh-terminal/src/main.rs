mod session;
mod terminal;

use gpui::{
    App, Application, AsyncApp, Bounds, ClipboardItem, Context, FontStyle, FontWeight,
    HighlightStyle, KeyBinding, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, StyledText, Timer,
    UnderlineStyle, WeakEntity, Window, WindowBounds, WindowOptions, actions, div, font,
    prelude::*, px, relative, rgb, size,
};
use std::{
    ops::Range,
    time::{Duration, Instant},
};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

use crate::session::LocalShellSession;
use crate::terminal::{TerminalHighlight, TerminalSnapshot, TerminalState};

actions!(terminal, [Quit]);

// 终端渲染参数；resize 估算与 UI 样式必须保持一致，避免 PTY 网格偏小/偏大。
const TERMINAL_PRIMARY_FONT_FAMILY: &str = "CaskaydiaCove Nerd Font";
const TERMINAL_FALLBACK_FONT_FAMILY: &str = "Menlo";
const TERMINAL_FONT_SIZE_PX: f32 = 13.0;
// 终端网格建议使用 1.0 行高，避免 box-drawing 竖线在行间出现断裂。
const TERMINAL_LINE_HEIGHT: f32 = 1.2;
const TERMINAL_FALLBACK_CHAR_WIDTH_FACTOR: f32 = 0.60; // 文本系统不可用时回退值
const TERMINAL_TOP_BAR_HEIGHT_PX: f32 = 34.0;
const TERMINAL_CONTENT_PADDING_PX: f32 = 12.0; // 对应 p_3
const TERMINAL_WIDTH_SAFETY_PX: f32 = 12.0;

fn compute_terminal_grid(
    window_width_px: f32,
    window_height_px: f32,
    cell_width: f32,
    cell_height: f32,
) -> (u16, u16) {
    // 只把真实文本区用于网格计算，避免“UI 很大但 shell 网格偏小”。
    let usable_width =
        (window_width_px - (TERMINAL_CONTENT_PADDING_PX * 2.0) - TERMINAL_WIDTH_SAFETY_PX).max(1.0);
    let usable_height =
        (window_height_px - TERMINAL_TOP_BAR_HEIGHT_PX - (TERMINAL_CONTENT_PADDING_PX * 2.0))
            .max(1.0);

    let cols = (usable_width / cell_width).floor().max(20.0) as u16;
    let rows = (usable_height / cell_height).floor().max(8.0) as u16;
    (cols, rows)
}

fn terminal_cell_metrics(window: &Window, font_family: &str) -> (f32, f32) {
    let text_system = window.text_system();
    let font_id = text_system.resolve_font(&font(font_family.to_string()));

    // 直接读取字体的 ch advance（'0' 的前进宽度），避免固定比例近似误差。
    let cell_width = text_system
        .ch_advance(font_id, px(TERMINAL_FONT_SIZE_PX))
        .map(|pixels| pixels / px(1.0))
        .unwrap_or(TERMINAL_FONT_SIZE_PX * TERMINAL_FALLBACK_CHAR_WIDTH_FACTOR)
        .max(1.0);

    // 行高由 UI 样式决定；按像素向上取整以避免累计溢出。
    let cell_height = (TERMINAL_FONT_SIZE_PX * TERMINAL_LINE_HEIGHT)
        .ceil()
        .max(1.0);
    (cell_width, cell_height)
}

// GPUI 视图层：负责输入、尺寸同步和把终端快照渲染到窗口。
struct TerminalView {
    session: LocalShellSession,
    terminal: TerminalState,
    cols: u16,
    rows: u16,
    status: String,
    blink_started_at: Instant,
    font_family: &'static str,
    font_checked: bool,
    scrollback_offset: usize,
    session_ended_notified: bool,
    selection_anchor: Option<(u16, u16)>,
    selection_cursor: Option<(u16, u16)>,
}

impl TerminalView {
    fn new(session: LocalShellSession) -> Self {
        Self {
            session,
            terminal: TerminalState::new(5000, 400),
            cols: 120,
            rows: 40,
            status: "Backend: local PTY (GPUI)".to_string(),
            blink_started_at: Instant::now(),
            font_family: TERMINAL_PRIMARY_FONT_FAMILY,
            font_checked: false,
            scrollback_offset: 0,
            session_ended_notified: false,
            selection_anchor: None,
            selection_cursor: None,
        }
    }

    fn ensure_terminal_font(&mut self, window: &Window) {
        if self.font_checked {
            return;
        }

        let has_primary = window
            .text_system()
            .all_font_names()
            .iter()
            .any(|name| name == TERMINAL_PRIMARY_FONT_FAMILY);

        if !has_primary {
            self.font_family = TERMINAL_FALLBACK_FONT_FAMILY;
            self.status = format!(
                "Backend: local PTY (GPUI), font fallback: {}",
                TERMINAL_FALLBACK_FONT_FAMILY
            );
        }

        self.font_checked = true;
    }

    fn drain_output(&mut self) {
        // 非阻塞地尽可能清空 PTY 输出，保持 UI 与后端状态同步。
        while let Some(chunk) = self.session.try_read() {
            self.terminal.feed(&chunk);
        }

        if self.session.is_closed() && !self.session_ended_notified {
            self.status = "Session ended (shell exited). Press Cmd+Q to close.".to_string();
            self.session_ended_notified = true;
            std::process::exit(0);
        }
    }

    fn scrollback_by_lines(&mut self, lines: i32) {
        if lines == 0 {
            return;
        }

        if lines > 0 {
            self.scrollback_offset = self.scrollback_offset.saturating_add(lines as usize);
        } else {
            self.scrollback_offset = self
                .scrollback_offset
                .saturating_sub(lines.unsigned_abs() as usize);
        }

        self.scrollback_offset = self
            .terminal
            .clamp_scrollback_offset(self.scrollback_offset);
    }

    fn begin_selection(&mut self, position: Point<Pixels>, window: &Window) {
        let (cell_width, cell_height) = terminal_cell_metrics(window, self.font_family);
        if let Some((col, row)) =
            viewport_cell_from_position(position, cell_width, cell_height, self.cols, self.rows)
        {
            self.selection_anchor = Some((row, col));
            self.selection_cursor = Some((row, col));
        }
    }

    fn update_selection(&mut self, position: Point<Pixels>, window: &Window) {
        if self.selection_anchor.is_none() {
            return;
        }

        let (cell_width, cell_height) = terminal_cell_metrics(window, self.font_family);
        if let Some((col, row)) =
            viewport_cell_from_position(position, cell_width, cell_height, self.cols, self.rows)
        {
            self.selection_cursor = Some((row, col));
        }
    }

    fn finish_selection_copy(&mut self, cx: &mut App) {
        let Some(start) = self.selection_anchor else {
            return;
        };
        let Some(end) = self.selection_cursor else {
            return;
        };

        if start == end {
            self.selection_anchor = None;
            self.selection_cursor = None;
            return;
        }

        let text = self
            .terminal
            .selected_text(start, end, self.scrollback_offset);
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            self.status = format!("Copied {} chars", text.chars().count());
        }

        self.selection_anchor = None;
        self.selection_cursor = None;
    }

    fn title_text(&self) -> String {
        if self.scrollback_offset == 0 {
            format!("GPUI Terminal PoC   {}x{}", self.cols, self.rows)
        } else {
            format!(
                "GPUI Terminal PoC   {}x{}   history: -{}",
                self.cols, self.rows, self.scrollback_offset
            )
        }
    }

    fn send_input(&mut self, bytes: &[u8]) {
        if let Err(err) = self.session.send_input(bytes) {
            self.status = format!("Input error: {err}");
        }
    }

    fn paste_from_clipboard(&mut self, cx: &mut App) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let Some(text) = item.text() else {
            return;
        };

        if text.is_empty() {
            return;
        }

        if self.terminal.bracketed_paste_mode() {
            let mut payload = Vec::with_capacity(text.len() + 16);
            payload.extend_from_slice(b"\x1b[200~");
            payload.extend_from_slice(text.as_bytes());
            payload.extend_from_slice(b"\x1b[201~");
            self.send_input(&payload);
        } else {
            self.send_input(text.as_bytes());
        }
    }

    fn sync_pty_size(&mut self, window: &mut Window) {
        self.ensure_terminal_font(window);

        // 根据窗口真实可用区域估算 cols/rows，并在变化时同步到 PTY。
        let content_size = window.bounds().size;
        let window_width_px = content_size.width / px(1.0);
        let window_height_px = content_size.height / px(1.0);
        let (cell_width, cell_height) = terminal_cell_metrics(window, self.font_family);
        let (new_cols, new_rows) =
            compute_terminal_grid(window_width_px, window_height_px, cell_width, cell_height);

        if new_cols != self.cols || new_rows != self.rows {
            self.cols = new_cols;
            self.rows = new_rows;

            if let Err(err) = self.session.resize(self.cols, self.rows) {
                self.status = format!("Resize error: {err}");
            }
        }

        // 终端状态机尺寸也需要同步，否则全屏 TUI 会出现错位或裁切。
        self.terminal.resize(self.cols, self.rows);
    }

    fn handle_keystroke(&mut self, keystroke: &Keystroke, cx: &mut App) {
        // 输入优先级：Ctrl 组合 -> 特殊键序列 -> 文本字符，避免键位冲突。
        if keystroke.modifiers.platform {
            if keystroke.key == "v" && !keystroke.modifiers.control && !keystroke.modifiers.alt {
                self.paste_from_clipboard(cx);
            }
            return;
        }

        if keystroke.modifiers.function {
            return;
        }

        if keystroke.modifiers.shift
            && !keystroke.modifiers.control
            && !keystroke.modifiers.alt
            && matches!(keystroke.key.as_str(), "pageup" | "pagedown")
        {
            let page_lines = self.rows as i32;
            if keystroke.key == "pageup" {
                self.scrollback_by_lines(page_lines);
            } else {
                self.scrollback_by_lines(-page_lines);
            }
            return;
        }

        // 任意发送到 PTY 的输入都会回到底部，保持交互可见。
        self.scrollback_offset = 0;

        let application_cursor = self.terminal.application_cursor_mode();

        if let Some(ctrl) = ctrl_byte_from_keystroke(keystroke) {
            self.send_input(&[ctrl]);
            return;
        }

        if let Some(seq) = special_key_sequence(
            &keystroke.key,
            keystroke.modifiers.shift,
            keystroke.modifiers.alt,
            application_cursor,
        ) {
            self.send_input(seq.as_bytes());
            return;
        }

        if let Some(key_char) = &keystroke.key_char {
            if keystroke.modifiers.alt {
                self.send_input(&[0x1b]);
            }
            self.send_input(key_char.as_bytes());
            return;
        }

        if keystroke.key.len() == 1
            && !keystroke.modifiers.control
            && !keystroke.modifiers.alt
            && let Some(ch) = keystroke.key.chars().next()
        {
            let mut buffer = [0_u8; 4];
            let encoded = ch.encode_utf8(&mut buffer);
            self.send_input(encoded.as_bytes());
        }

        // Keep cursor visible right after key input for better UX.
        self.blink_started_at = Instant::now();
    }

    fn should_show_cursor(&self) -> bool {
        // 500ms 半周期闪烁。
        let half_period_ms = 500;
        (self.blink_started_at.elapsed().as_millis() / half_period_ms) % 2 == 0
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.drain_output();
        self.sync_pty_size(window);
        self.scrollback_offset = self
            .terminal
            .clamp_scrollback_offset(self.scrollback_offset);
        let show_cursor = self.should_show_cursor();
        // 先生成“文本+样式区间”快照，再交给 GPUI 富文本渲染。
        let snapshot = self.terminal.snapshot_with_cursor(
            self.rows as usize,
            self.scrollback_offset,
            show_cursor,
        );
        let selection_range =
            selection_range_in_snapshot(&snapshot, self.selection_anchor, self.selection_cursor);

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x0b1020))
            .text_color(rgb(0xdbe7ff))
            .font_family(self.font_family)
            .text_size(px(TERMINAL_FONT_SIZE_PX))
            .line_height(relative(TERMINAL_LINE_HEIGHT))
            .child(
                div()
                    .flex_none()
                    .h(px(TERMINAL_TOP_BAR_HEIGHT_PX))
                    .px_3()
                    .bg(rgb(0x141a2b))
                    .border_b_1()
                    .border_color(rgb(0x29324d))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(self.title_text())
                    .child(self.status.clone()),
            )
            .child(
                div()
                    .id("terminal-output")
                    .size_full()
                    .p_3()
                    .overflow_hidden()
                    .on_scroll_wheel(cx.listener(
                        |this: &mut TerminalView, event: &ScrollWheelEvent, window, cx| {
                            let steps = scroll_lines_from_wheel(event);
                            if steps == 0 {
                                return;
                            }

                            let mouse_mode = this.terminal.mouse_protocol_mode();
                            if mouse_mode != MouseProtocolMode::None {
                                let (cell_width, cell_height) =
                                    terminal_cell_metrics(window, this.font_family);
                                if let Some((col, row)) = mouse_cell_from_wheel_event(
                                    event,
                                    cell_width,
                                    cell_height,
                                    this.cols,
                                    this.rows,
                                ) {
                                    let direction_up = steps < 0;
                                    let count = steps.unsigned_abs().max(1) as usize;
                                    for _ in 0..count {
                                        let sequence = encode_wheel_mouse_event(
                                            mouse_mode,
                                            this.terminal.mouse_protocol_encoding(),
                                            col,
                                            row,
                                            direction_up,
                                            event,
                                        );
                                        this.send_input(&sequence);
                                    }
                                }
                            } else {
                                this.scrollback_by_lines(steps);
                                cx.notify();
                            }
                        },
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(
                            |this: &mut TerminalView, event: &MouseDownEvent, window, cx| {
                                let mouse_mode = this.terminal.mouse_protocol_mode();
                                if mouse_mode != MouseProtocolMode::None {
                                    let (cell_width, cell_height) =
                                        terminal_cell_metrics(window, this.font_family);
                                    if let Some((col, row)) = terminal_cell_from_position_1based(
                                        event.position,
                                        cell_width,
                                        cell_height,
                                        this.cols,
                                        this.rows,
                                    ) {
                                        let sequence = encode_mouse_button_event(
                                            mouse_mode,
                                            this.terminal.mouse_protocol_encoding(),
                                            col,
                                            row,
                                            MouseButtonEventKind::PressLeft,
                                            event.modifiers,
                                        );
                                        this.send_input(&sequence);
                                    }
                                    return;
                                }
                                this.begin_selection(event.position, window);
                                cx.notify();
                            },
                        ),
                    )
                    .on_mouse_move(cx.listener(
                        |this: &mut TerminalView, event: &MouseMoveEvent, window, cx| {
                            let mouse_mode = this.terminal.mouse_protocol_mode();
                            if mouse_mode != MouseProtocolMode::None {
                                if event.dragging()
                                    && matches!(
                                        mouse_mode,
                                        MouseProtocolMode::ButtonMotion
                                            | MouseProtocolMode::AnyMotion
                                    )
                                {
                                    let (cell_width, cell_height) =
                                        terminal_cell_metrics(window, this.font_family);
                                    if let Some((col, row)) = terminal_cell_from_position_1based(
                                        event.position,
                                        cell_width,
                                        cell_height,
                                        this.cols,
                                        this.rows,
                                    ) {
                                        let sequence = encode_mouse_button_event(
                                            mouse_mode,
                                            this.terminal.mouse_protocol_encoding(),
                                            col,
                                            row,
                                            MouseButtonEventKind::DragLeft,
                                            event.modifiers,
                                        );
                                        this.send_input(&sequence);
                                    }
                                }
                                return;
                            }

                            if event.dragging() {
                                this.update_selection(event.position, window);
                                cx.notify();
                            }
                        },
                    ))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(
                            |this: &mut TerminalView, event: &MouseUpEvent, window, cx| {
                                let mouse_mode = this.terminal.mouse_protocol_mode();
                                if mouse_mode != MouseProtocolMode::None {
                                    let (cell_width, cell_height) =
                                        terminal_cell_metrics(window, this.font_family);
                                    if let Some((col, row)) = terminal_cell_from_position_1based(
                                        event.position,
                                        cell_width,
                                        cell_height,
                                        this.cols,
                                        this.rows,
                                    ) {
                                        let sequence = encode_mouse_button_event(
                                            mouse_mode,
                                            this.terminal.mouse_protocol_encoding(),
                                            col,
                                            row,
                                            MouseButtonEventKind::ReleaseLeft,
                                            event.modifiers,
                                        );
                                        this.send_input(&sequence);
                                    }
                                    return;
                                }

                                this.update_selection(event.position, window);
                                this.finish_selection_copy(cx);
                                cx.notify();
                            },
                        ),
                    )
                    .child(styled_terminal_snapshot(snapshot, selection_range)),
            )
    }
}

fn selection_range_in_snapshot(
    snapshot: &TerminalSnapshot,
    anchor: Option<(u16, u16)>,
    cursor: Option<(u16, u16)>,
) -> Option<Range<usize>> {
    let (start, end) = match (anchor, cursor) {
        (Some(a), Some(c)) => (a, c),
        _ => return None,
    };

    if snapshot.cols == 0 || snapshot.rows == 0 || snapshot.cell_offsets.len() < 2 {
        return None;
    }

    let (mut start_row, mut start_col) = (start.0 as usize, start.1 as usize);
    let (mut end_row, mut end_col) = (end.0 as usize, end.1 as usize);
    start_row = start_row.min(snapshot.rows - 1);
    start_col = start_col.min(snapshot.cols - 1);
    end_row = end_row.min(snapshot.rows - 1);
    end_col = end_col.min(snapshot.cols - 1);

    if (start_row, start_col) > (end_row, end_col) {
        std::mem::swap(&mut start_row, &mut end_row);
        std::mem::swap(&mut start_col, &mut end_col);
    }

    let start_index = start_row * snapshot.cols + start_col;
    let end_inclusive_index = end_row * snapshot.cols + end_col;
    let end_exclusive_index = (end_inclusive_index + 1).min(snapshot.cell_offsets.len() - 1);

    let start_offset = snapshot.cell_offsets[start_index];
    let end_offset = snapshot.cell_offsets[end_exclusive_index];
    (start_offset < end_offset).then_some(start_offset..end_offset)
}

fn scroll_lines_from_wheel(event: &ScrollWheelEvent) -> i32 {
    let raw_steps = match event.delta {
        ScrollDelta::Lines(delta) => delta.y.round() as i32,
        ScrollDelta::Pixels(delta) => ((delta.y / px(20.0)).round()) as i32,
    };

    // macOS 的滚轮 delta 符号与终端常见语义（如 iTerm2）相反，统一在此处校正。
    if cfg!(target_os = "macos") {
        -raw_steps
    } else {
        raw_steps
    }
}

fn mouse_cell_from_wheel_event(
    event: &ScrollWheelEvent,
    cell_width: f32,
    cell_height: f32,
    cols: u16,
    rows: u16,
) -> Option<(u16, u16)> {
    if cols == 0 || rows == 0 {
        return None;
    }

    let mouse_x = (event.position.x / px(1.0)) - TERMINAL_CONTENT_PADDING_PX;
    let mouse_y =
        (event.position.y / px(1.0)) - TERMINAL_TOP_BAR_HEIGHT_PX - TERMINAL_CONTENT_PADDING_PX;

    if mouse_x < 0.0 || mouse_y < 0.0 {
        return None;
    }

    let col = ((mouse_x / cell_width).floor() as i32 + 1).clamp(1, cols as i32) as u16;
    let row = ((mouse_y / cell_height).floor() as i32 + 1).clamp(1, rows as i32) as u16;
    Some((col, row))
}

fn terminal_cell_from_position_1based(
    position: Point<Pixels>,
    cell_width: f32,
    cell_height: f32,
    cols: u16,
    rows: u16,
) -> Option<(u16, u16)> {
    if cols == 0 || rows == 0 {
        return None;
    }

    let mouse_x = (position.x / px(1.0)) - TERMINAL_CONTENT_PADDING_PX;
    let mouse_y = (position.y / px(1.0)) - TERMINAL_TOP_BAR_HEIGHT_PX - TERMINAL_CONTENT_PADDING_PX;

    if mouse_x < 0.0 || mouse_y < 0.0 {
        return None;
    }

    let col = ((mouse_x / cell_width).floor() as i32 + 1).clamp(1, cols as i32) as u16;
    let row = ((mouse_y / cell_height).floor() as i32 + 1).clamp(1, rows as i32) as u16;
    Some((col, row))
}

fn viewport_cell_from_position(
    position: Point<Pixels>,
    cell_width: f32,
    cell_height: f32,
    cols: u16,
    rows: u16,
) -> Option<(u16, u16)> {
    if cols == 0 || rows == 0 {
        return None;
    }

    let mouse_x = (position.x / px(1.0)) - TERMINAL_CONTENT_PADDING_PX;
    let mouse_y = (position.y / px(1.0)) - TERMINAL_TOP_BAR_HEIGHT_PX - TERMINAL_CONTENT_PADDING_PX;

    if mouse_x < 0.0 || mouse_y < 0.0 {
        return None;
    }

    let col = (mouse_x / cell_width).floor().clamp(0.0, (cols - 1) as f32) as u16;
    let row = (mouse_y / cell_height)
        .floor()
        .clamp(0.0, (rows - 1) as f32) as u16;
    Some((col, row))
}

fn encode_wheel_mouse_event(
    mode: MouseProtocolMode,
    encoding: MouseProtocolEncoding,
    col: u16,
    row: u16,
    direction_up: bool,
    event: &ScrollWheelEvent,
) -> Vec<u8> {
    if mode == MouseProtocolMode::None {
        return Vec::new();
    }

    let mut cb = if direction_up { 64_u16 } else { 65_u16 };
    if event.modifiers.shift {
        cb += 4;
    }
    if event.modifiers.alt {
        cb += 8;
    }
    if event.modifiers.control {
        cb += 16;
    }

    match encoding {
        MouseProtocolEncoding::Sgr => format!("\x1b[<{};{};{}M", cb, col, row).into_bytes(),
        MouseProtocolEncoding::Default | MouseProtocolEncoding::Utf8 => {
            let cb_byte = (32 + cb.min(223)) as u8;
            let col_byte = (32 + col.min(223)) as u8;
            let row_byte = (32 + row.min(223)) as u8;
            vec![0x1b, b'[', b'M', cb_byte, col_byte, row_byte]
        }
    }
}

#[derive(Clone, Copy)]
enum MouseButtonEventKind {
    PressLeft,
    DragLeft,
    ReleaseLeft,
}

fn encode_mouse_button_event(
    mode: MouseProtocolMode,
    encoding: MouseProtocolEncoding,
    col: u16,
    row: u16,
    kind: MouseButtonEventKind,
    modifiers: gpui::Modifiers,
) -> Vec<u8> {
    if mode == MouseProtocolMode::None {
        return Vec::new();
    }

    let mut cb = match kind {
        MouseButtonEventKind::PressLeft => 0_u16,
        MouseButtonEventKind::DragLeft => 32_u16,
        MouseButtonEventKind::ReleaseLeft => 3_u16,
    };

    if modifiers.shift {
        cb += 4;
    }
    if modifiers.alt {
        cb += 8;
    }
    if modifiers.control {
        cb += 16;
    }

    match encoding {
        MouseProtocolEncoding::Sgr => {
            let suffix = if matches!(kind, MouseButtonEventKind::ReleaseLeft) {
                'm'
            } else {
                'M'
            };
            format!("\x1b[<{};{};{}{}", cb, col, row, suffix).into_bytes()
        }
        MouseProtocolEncoding::Default | MouseProtocolEncoding::Utf8 => {
            let cb_byte = (32 + cb.min(223)) as u8;
            let col_byte = (32 + col.min(223)) as u8;
            let row_byte = (32 + row.min(223)) as u8;
            vec![0x1b, b'[', b'M', cb_byte, col_byte, row_byte]
        }
    }
}

fn styled_terminal_snapshot(
    snapshot: TerminalSnapshot,
    selection_range: Option<Range<usize>>,
) -> StyledText {
    let text_len = snapshot.text.len();
    let mut styled = StyledText::new(snapshot.text);

    let terminal_highlights = snapshot
        .highlights
        .into_iter()
        .map(|highlight| {
            let style = style_from_terminal_highlight(&highlight);
            (highlight.range, style)
        })
        .collect::<Vec<_>>();

    let merged_highlights = merge_selection_highlights(
        terminal_highlights,
        selection_range,
        text_len,
        rgb(0x2d4f8f).into(),
    );

    if !merged_highlights.is_empty() {
        styled = styled.with_highlights(merged_highlights);
    }

    styled
}

fn merge_selection_highlights(
    terminal: Vec<(Range<usize>, HighlightStyle)>,
    selection: Option<Range<usize>>,
    text_len: usize,
    selection_bg: gpui::Hsla,
) -> Vec<(Range<usize>, HighlightStyle)> {
    if text_len == 0 {
        return Vec::new();
    }

    let mut boundaries = vec![0, text_len];
    for (range, _) in &terminal {
        boundaries.push(range.start.min(text_len));
        boundaries.push(range.end.min(text_len));
    }

    let selection = selection.map(|mut range| {
        range.start = range.start.min(text_len);
        range.end = range.end.min(text_len);
        range
    });

    if let Some(range) = &selection {
        boundaries.push(range.start);
        boundaries.push(range.end);
    }

    boundaries.sort_unstable();
    boundaries.dedup();

    let mut result: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    let mut term_ix = 0;

    for pair in boundaries.windows(2) {
        let start = pair[0];
        let end = pair[1];
        if start >= end {
            continue;
        }

        while term_ix < terminal.len() && terminal[term_ix].0.end <= start {
            term_ix += 1;
        }

        let mut style = HighlightStyle::default();
        if term_ix < terminal.len() {
            let (range, term_style) = &terminal[term_ix];
            if range.start <= start && start < range.end {
                style = *term_style;
            }
        }

        if let Some(sel) = &selection
            && sel.start <= start
            && start < sel.end
        {
            style.background_color = Some(selection_bg);
        }

        if style != HighlightStyle::default() {
            if let Some((last_range, last_style)) = result.last_mut()
                && *last_style == style
                && last_range.end == start
            {
                last_range.end = end;
            } else {
                result.push((start..end, style));
            }
        }
    }

    result
}

fn style_from_terminal_highlight(highlight: &TerminalHighlight) -> HighlightStyle {
    let mut style = HighlightStyle::default();

    if let Some([r, g, b]) = highlight.fg {
        let rgb24 = ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
        style = style.highlight(HighlightStyle::from(rgb(rgb24)));
    }

    if let Some([r, g, b]) = highlight.bg {
        let rgb24 = ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
        style.background_color = Some(rgb(rgb24).into());
    }

    if highlight.bold {
        style.font_weight = Some(FontWeight::BOLD);
    }

    if highlight.italic {
        style.font_style = Some(FontStyle::Italic);
    }

    if highlight.underline {
        style.underline = Some(UnderlineStyle::default());
    }

    if highlight.faint {
        // 用 fade_out 呈现“幽灵提示”的浅色效果。
        style.fade_out = Some(0.6);
    }

    style
}

fn special_key_sequence(
    key: &str,
    shift: bool,
    alt: bool,
    application_cursor: bool,
) -> Option<&'static str> {
    // 将常见按键转换为终端控制序列；Left 键会透传给 shell 处理“接受建议”等行为。
    match key {
        "enter" => Some("\r"),
        "tab" => {
            if shift {
                Some("\x1b[Z")
            } else {
                Some("\t")
            }
        }
        // 大多数 xterm/终端默认将 Backspace 发送为 DEL(0x7f)。
        "backspace" => Some("\x7f"),
        "up" => {
            // Alt+方向键使用 xterm CSI 修饰符，便于 zellij 等 TUI 识别 pane 切换。
            if alt {
                Some("\x1b[1;3A")
            } else if application_cursor {
                Some("\x1bOA")
            } else {
                Some("\x1b[A")
            }
        }
        "down" => {
            if alt {
                Some("\x1b[1;3B")
            } else if application_cursor {
                Some("\x1bOB")
            } else {
                Some("\x1b[B")
            }
        }
        "right" => {
            if alt {
                Some("\x1b[1;3C")
            } else if application_cursor {
                Some("\x1bOC")
            } else {
                Some("\x1b[C")
            }
        }
        "left" => {
            if alt {
                Some("\x1b[1;3D")
            } else if application_cursor {
                Some("\x1bOD")
            } else {
                Some("\x1b[D")
            }
        }
        "home" => {
            if application_cursor {
                Some("\x1bOH")
            } else {
                Some("\x1b[H")
            }
        }
        "end" => {
            if application_cursor {
                Some("\x1bOF")
            } else {
                Some("\x1b[F")
            }
        }
        "delete" => Some("\x1b[3~"),
        "escape" => Some("\x1b"),
        "pageup" => {
            if shift {
                Some("\x1b[5;2~")
            } else {
                Some("\x1b[5~")
            }
        }
        "pagedown" => {
            if shift {
                Some("\x1b[6;2~")
            } else {
                Some("\x1b[6~")
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TERMINAL_FALLBACK_CHAR_WIDTH_FACTOR, TERMINAL_FONT_SIZE_PX, TERMINAL_LINE_HEIGHT,
        compute_terminal_grid, special_key_sequence,
    };

    #[test]
    fn alt_arrows_use_xterm_csi_modifier() {
        assert_eq!(
            special_key_sequence("up", false, true, false),
            Some("\x1b[1;3A")
        );
        assert_eq!(
            special_key_sequence("down", false, true, false),
            Some("\x1b[1;3B")
        );
        assert_eq!(
            special_key_sequence("right", false, true, false),
            Some("\x1b[1;3C")
        );
        assert_eq!(
            special_key_sequence("left", false, true, false),
            Some("\x1b[1;3D")
        );
    }

    #[test]
    fn regular_and_application_cursor_arrows_keep_existing_behavior() {
        assert_eq!(
            special_key_sequence("left", false, false, false),
            Some("\x1b[D")
        );
        assert_eq!(
            special_key_sequence("left", false, false, true),
            Some("\x1bOD")
        );
    }

    #[test]
    fn shift_page_keys_remain_unchanged() {
        assert_eq!(
            special_key_sequence("pageup", true, false, false),
            Some("\x1b[5;2~")
        );
        assert_eq!(
            special_key_sequence("pagedown", true, false, false),
            Some("\x1b[6;2~")
        );
    }

    #[test]
    fn resize_estimation_matches_terminal_content_area() {
        let cell_width = TERMINAL_FONT_SIZE_PX * TERMINAL_FALLBACK_CHAR_WIDTH_FACTOR;
        let cell_height = (TERMINAL_FONT_SIZE_PX * TERMINAL_LINE_HEIGHT).ceil();
        let (cols, rows) = compute_terminal_grid(1024.0, 720.0, cell_width, cell_height);
        assert_eq!(cols, 126);
        assert_eq!(rows, 50);
    }

    #[test]
    fn resize_estimation_has_sane_minimums() {
        let (cols, rows) = compute_terminal_grid(10.0, 10.0, 7.8, 16.0);
        assert_eq!(cols, 20);
        assert_eq!(rows, 8);
    }
}

fn ctrl_byte_from_keystroke(keystroke: &Keystroke) -> Option<u8> {
    // ASCII 控制键映射：Ctrl+A..Z -> 0x01..0x1A，补充常见标点组合。
    if !keystroke.modifiers.control || keystroke.modifiers.platform || keystroke.modifiers.alt {
        return None;
    }

    if keystroke.key.len() == 1 {
        let b = keystroke.key.as_bytes()[0].to_ascii_lowercase();
        if b.is_ascii_lowercase() {
            return Some(b - b'a' + 1);
        }
    }

    match keystroke.key.as_str() {
        "space" => Some(0x00),
        "[" => Some(0x1b),
        "\\" => Some(0x1c),
        "]" => Some(0x1d),
        "^" => Some(0x1e),
        "_" => Some(0x1f),
        "?" => Some(0x7f),
        _ => None,
    }
}

async fn refresh_loop(this: WeakEntity<TerminalView>, mut app: AsyncApp) {
    // 周期刷新兜底：即便没有输入事件，也确保输出和光标闪烁持续更新。
    loop {
        Timer::after(Duration::from_millis(16)).await;

        if this
            .update(&mut app, |this, cx| {
                this.drain_output();
                cx.notify();
            })
            .is_err()
        {
            break;
        }
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1024.0), px(720.0)), cx);

        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    cx.new(|cx| {
                        let session = LocalShellSession::spawn_default(120, 40)
                            .expect("failed to start local PTY session");
                        let mut view = TerminalView::new(session);

                        view.sync_pty_size(window);

                        cx.observe_keystrokes(|this: &mut TerminalView, event, _window, cx| {
                            this.handle_keystroke(&event.keystroke, cx);
                            this.drain_output();
                            cx.notify();
                        })
                        .detach();

                        cx.observe_window_bounds(window, |this: &mut TerminalView, window, cx| {
                            this.sync_pty_size(window);
                            cx.notify();
                        })
                        .detach();

                        cx.spawn(
                            async move |this: WeakEntity<TerminalView>, cx: &mut AsyncApp| {
                                refresh_loop(this, cx.clone()).await;
                            },
                        )
                        .detach();

                        view
                    })
                },
            )
            .expect("failed to open GPUI window");

        window
            .update(cx, |view, window, cx| {
                view.sync_pty_size(window);
                cx.activate(true);
            })
            .ok();

        cx.bind_keys([KeyBinding::new("secondary-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());
    });
}
