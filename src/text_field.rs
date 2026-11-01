mod gap_string;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::{Buffer, CellWidth},
    layout::{Offset, Rect},
    widgets::StatefulWidget,
};
use unicode_segmentation::UnicodeSegmentation;

pub use gap_string::GapString;

pub struct TextFieldState {
    pub text: GapString,
    pub cursor_col: u16,
    pub cursor_row: u16,
    pub cursor_idx: usize,
    pub scroll_x: u16,
    pub scroll_y: u16,
}

impl TextFieldState {
    pub fn new() -> Self {
        String::new().into()
    }

    pub fn cursor_offset(&self) -> Offset {
        Offset::new(
            (self.cursor_col - self.scroll_x) as i32,
            (self.cursor_row - self.scroll_y) as i32,
        )
    }

    pub fn handle_input(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Char(c) => self.insert(c),
            KeyCode::Enter => self.insert('\n'),
            KeyCode::Left => self.move_cursor_left(),
            KeyCode::Right => self.move_cursor_right(),
            KeyCode::Up => self.move_cursor_up(),
            KeyCode::Down => self.move_cursor_down(),
            KeyCode::Backspace => self.delete(1),
            _ => {}
        }
    }

    pub fn move_cursor_left(&mut self) {
        let mut graphemes = self.text.graphemes(..self.cursor_idx);
        let mut grapheme = String::new();
        graphemes.read_next_back(&mut grapheme);
        if grapheme.is_empty() {
            return;
        }
        self.cursor_idx -= grapheme.len();

        if grapheme.contains('\n') {
            self.cursor_row -= 1;
            self.cursor_col = 0;
            loop {
                grapheme.clear();
                graphemes.read_next_back(&mut grapheme);
                if grapheme.is_empty() || grapheme.contains('\n') {
                    break;
                }
                self.cursor_col += grapheme.cell_width();
                grapheme.clear();
            }
        } else {
            self.cursor_col -= grapheme.cell_width();
        }
    }

    pub fn move_cursor_right(&mut self) {
        let mut graphemes = self.text.graphemes(self.cursor_idx..);
        let mut grapheme = String::new();
        graphemes.read_next(&mut grapheme);
        self.update_cursor_with_grapheme(&grapheme);
    }

    pub fn move_cursor_up(&mut self) {
        let col = self.cursor_col;
        let row = self.cursor_row;
        while self.cursor_row == row && self.cursor_idx > 0 {
            self.move_cursor_left();
        }
        while self.cursor_col > col {
            self.move_cursor_left();
        }
    }

    pub fn move_cursor_down(&mut self) {
        let col = self.cursor_col;
        let row = self.cursor_row;
        while self.cursor_row == row && self.cursor_idx < self.text.len() {
            self.move_cursor_right();
        }
        let row = self.cursor_row;
        while self.cursor_col < col && self.cursor_idx < self.text.len() {
            self.move_cursor_right();
            if self.cursor_row != row {
                self.move_cursor_left();
                break;
            }
        }
    }

    fn update_cursor_with_grapheme(&mut self, grapheme: &str) {
        self.cursor_idx += grapheme.len();
        if grapheme.contains('\n') {
            self.cursor_row += 1;
            self.cursor_col = 0;
        } else {
            self.cursor_col += grapheme.cell_width();
        }
    }

    pub fn insert(&mut self, c: char) {
        let mut buf = [0; 4];
        self.insert_str(c.encode_utf8(&mut buf));
    }

    pub fn insert_str(&mut self, c: &str) {
        self.text.move_cursor(self.cursor_idx);
        self.text.insert_str(c);

        for grapheme in c.graphemes(true) {
            self.update_cursor_with_grapheme(grapheme);
        }
    }

    pub fn delete(&mut self, n: usize) {
        let start = self.cursor_idx;
        for _ in 0..n {
            self.move_cursor_left();
        }
        self.text.move_cursor(start);
        self.text.delete(start - self.cursor_idx);
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor_col = 0;
        self.cursor_row = 0;
        self.cursor_idx = 0;
    }
}

impl From<String> for TextFieldState {
    fn from(value: String) -> Self {
        Self {
            text: value.into(),
            cursor_col: 0,
            cursor_row: 0,
            cursor_idx: 0,
            scroll_x: 0,
            scroll_y: 0,
        }
    }
}

#[derive(Debug, Default)]
pub struct TextField {}

impl TextField {
    pub fn new() -> Self {
        Self::default()
    }
}

impl StatefulWidget for TextField {
    type State = TextFieldState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if state.cursor_row < state.scroll_y {
            state.scroll_y = state.cursor_row;
        }
        if state.cursor_row >= state.scroll_y + area.height {
            state.scroll_y = state.cursor_row - area.height + 1;
        }
        if state.cursor_col < state.scroll_x {
            state.scroll_x = state.cursor_col
        }
        if state.cursor_col >= state.scroll_x + area.width {
            state.scroll_x = state.cursor_col - area.width + 1
        }

        let mut x = 0;
        let mut y = 0;

        let mut graphemes = state.text.graphemes(..);
        let mut grapheme = String::new();
        while y < state.scroll_y + area.height {
            grapheme.clear();
            graphemes.read_next(&mut grapheme);
            if grapheme.is_empty() {
                break;
            }

            if grapheme.contains('\n') {
                x = 0;
                y += 1;
            } else {
                if x >= state.scroll_x && y >= state.scroll_y {
                    let cell_x = area.x + x - state.scroll_x;
                    let cell_y = area.y + y - state.scroll_y;
                    if cell_x < area.right() {
                        buf.cell_mut((cell_x, cell_y))
                            .unwrap()
                            .set_symbol(&grapheme);
                    }
                }
                x += grapheme.cell_width();
            }
        }
    }
}
