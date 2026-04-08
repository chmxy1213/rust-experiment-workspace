use std::ops::Range;

use vt100::{Color, MouseProtocolEncoding, MouseProtocolMode, Parser};

// 渲染层消费的扁平快照：纯文本 + 样式区间。
#[derive(Clone, Debug, Default)]
pub struct TerminalSnapshot {
    pub text: String,
    pub highlights: Vec<TerminalHighlight>,
    pub cols: usize,
    pub rows: usize,
    // 每个单元格在 text 中的起始 byte 偏移（行优先），末尾额外带一个哨兵偏移。
    pub cell_offsets: Vec<usize>,
}

// 对应文本区间的样式信息，用于高亮和“幽灵提示”淡显。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalHighlight {
    pub range: Range<usize>,
    pub fg: Option<[u8; 3]>,
    pub bg: Option<[u8; 3]>,
    pub faint: bool,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

// 使用 vt100 作为核心终端状态机，提升 TUI（zellij/vim/htop）兼容性。
pub struct TerminalState {
    parser: Parser,
}

impl TerminalState {
    pub fn new(scrollback_len: usize, initial_cols: usize) -> Self {
        // 初始尺寸会在窗口创建后立即被 resize 同步。
        let cols = initial_cols.clamp(20, 512) as u16;
        Self {
            parser: Parser::new(40, cols, scrollback_len),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }

        let screen = self.parser.screen_mut();
        let (current_rows, current_cols) = screen.size();
        if current_rows != rows || current_cols != cols {
            screen.set_size(rows, cols);
        }
    }

    pub fn application_cursor_mode(&self) -> bool {
        self.parser.screen().application_cursor()
    }

    pub fn bracketed_paste_mode(&self) -> bool {
        self.parser.screen().bracketed_paste()
    }

    pub fn mouse_protocol_mode(&self) -> MouseProtocolMode {
        self.parser.screen().mouse_protocol_mode()
    }

    pub fn mouse_protocol_encoding(&self) -> MouseProtocolEncoding {
        self.parser.screen().mouse_protocol_encoding()
    }

    pub fn clamp_scrollback_offset(&mut self, scrollback_offset: usize) -> usize {
        let screen = self.parser.screen_mut();
        let original_offset = screen.scrollback();
        screen.set_scrollback(scrollback_offset);
        let clamped = screen.scrollback();
        screen.set_scrollback(original_offset);
        clamped
    }

    pub fn selected_text(
        &mut self,
        start: (u16, u16),
        end: (u16, u16),
        scrollback_offset: usize,
    ) -> String {
        let screen = self.parser.screen_mut();
        let original_offset = screen.scrollback();
        screen.set_scrollback(scrollback_offset);
        let (_, cols) = screen.size();

        let (mut start_row, mut start_col) = start;
        let (mut end_row, mut end_col) = end;
        if (start_row, start_col) > (end_row, end_col) {
            std::mem::swap(&mut start_row, &mut end_row);
            std::mem::swap(&mut start_col, &mut end_col);
        }

        // 选择语义按“结束单元格包含在内”，转换成 contents_between 所需的 end_col(开区间)。
        end_col = end_col.saturating_add(1).min(cols);

        let selected = screen.contents_between(start_row, start_col, end_row, end_col);
        screen.set_scrollback(original_offset);
        selected
    }

    pub fn snapshot_with_cursor(
        &mut self,
        max_visible_lines: usize,
        scrollback_offset: usize,
        show_cursor: bool,
    ) -> TerminalSnapshot {
        let screen = self.parser.screen_mut();
        let original_offset = screen.scrollback();
        screen.set_scrollback(scrollback_offset);
        let effective_offset = screen.scrollback();
        let (rows, cols) = screen.size();

        if rows == 0 || cols == 0 {
            screen.set_scrollback(original_offset);
            return TerminalSnapshot::default();
        }

        let total_rows = rows as usize;
        let total_cols = cols as usize;
        let visible_rows = total_rows.min(max_visible_lines.max(1));
        let start_row = total_rows.saturating_sub(visible_rows);

        let cursor = if effective_offset == 0 && show_cursor && !screen.hide_cursor() {
            Some(screen.cursor_position())
        } else {
            None
        };

        let mut text = String::new();
        let mut highlights = Vec::new();
        let mut cell_offsets = Vec::with_capacity((visible_rows * total_cols) + 1);

        for row in start_row..total_rows {
            for col in 0..total_cols {
                cell_offsets.push(text.len());

                let mut contents = " ";
                let mut fg = None;
                let mut bg = None;
                let mut faint = false;
                let mut bold = false;
                let mut italic = false;
                let mut underline = false;

                if let Some(cell) = screen.cell(row as u16, col as u16) {
                    if !cell.is_wide_continuation() && cell.has_contents() {
                        contents = cell.contents();
                    }

                    // inverse 模式下交换前景/背景，尽量贴近真实终端效果。
                    let (effective_fg, effective_bg) = if cell.inverse() {
                        (cell.bgcolor(), cell.fgcolor())
                    } else {
                        (cell.fgcolor(), cell.bgcolor())
                    };
                    fg = color_to_rgb(effective_fg);
                    bg = color_to_rgb(effective_bg);
                    faint = cell.dim();
                    bold = cell.bold();
                    italic = cell.italic();
                    underline = cell.underline();
                }

                if let Some((cursor_row, cursor_col)) = cursor
                    && cursor_row as usize == row
                    && cursor_col as usize == col
                {
                    contents = "|";
                    fg = None;
                    bg = None;
                    faint = false;
                    bold = false;
                    italic = false;
                    underline = false;
                }

                append_span(
                    &mut text,
                    &mut highlights,
                    contents,
                    fg,
                    bg,
                    faint,
                    bold,
                    italic,
                    underline,
                );
            }

            if row + 1 < total_rows {
                text.push('\n');
            }
        }

        // 哨兵：用于计算末尾单元格的结束偏移。
        cell_offsets.push(text.len());

        screen.set_scrollback(original_offset);

        TerminalSnapshot {
            text,
            highlights,
            cols: total_cols,
            rows: visible_rows,
            cell_offsets,
        }
    }
}

fn append_span(
    output: &mut String,
    highlights: &mut Vec<TerminalHighlight>,
    contents: &str,
    fg: Option<[u8; 3]>,
    bg: Option<[u8; 3]>,
    faint: bool,
    bold: bool,
    italic: bool,
    underline: bool,
) {
    let start = output.len();
    output.push_str(contents);
    let end = output.len();

    if fg.is_none() && bg.is_none() && !faint && !bold && !italic && !underline {
        return;
    }

    // 合并相邻同样式区间，减少 GPUI 文本 runs 数量。
    if let Some(last) = highlights.last_mut()
        && last.range.end == start
        && last.fg == fg
        && last.bg == bg
        && last.faint == faint
        && last.bold == bold
        && last.italic == italic
        && last.underline == underline
    {
        last.range.end = end;
    } else {
        highlights.push(TerminalHighlight {
            range: start..end,
            fg,
            bg,
            faint,
            bold,
            italic,
            underline,
        });
    }
}

fn color_to_rgb(color: Color) -> Option<[u8; 3]> {
    match color {
        Color::Default => None,
        Color::Idx(idx) => Some(ansi_256_color(idx)),
        Color::Rgb(r, g, b) => Some([r, g, b]),
    }
}

fn ansi_16_color(index: u8) -> [u8; 3] {
    match index {
        0 => [0, 0, 0],
        1 => [205, 49, 49],
        2 => [13, 188, 121],
        3 => [229, 229, 16],
        4 => [36, 114, 200],
        5 => [188, 63, 188],
        6 => [17, 168, 205],
        7 => [229, 229, 229],
        8 => [102, 102, 102],
        9 => [241, 76, 76],
        10 => [35, 209, 139],
        11 => [245, 245, 67],
        12 => [59, 142, 234],
        13 => [214, 112, 214],
        14 => [41, 184, 219],
        15 => [255, 255, 255],
        _ => [255, 255, 255],
    }
}

fn ansi_256_color(index: u8) -> [u8; 3] {
    match index {
        0..=15 => ansi_16_color(index),
        16..=231 => {
            // 6x6x6 颜色立方
            let cube = index - 16;
            let r = cube / 36;
            let g = (cube % 36) / 6;
            let b = cube % 6;
            let levels = [0, 95, 135, 175, 215, 255];
            [levels[r as usize], levels[g as usize], levels[b as usize]]
        }
        232..=255 => {
            // 24 级灰阶
            let gray = 8 + (index - 232) * 10;
            [gray, gray, gray]
        }
    }
}
