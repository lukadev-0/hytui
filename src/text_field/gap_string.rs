//! Implementation of a [gap buffer](https://en.wikipedia.org/wiki/Gap_buffer).

use std::{
    fmt,
    ops::{Bound, RangeBounds},
    range::Range,
};

use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};

#[derive(Debug, Clone)]
pub struct GapString {
    inner: String,
    gap: Range<usize>,
}

impl GapString {
    pub fn new() -> Self {
        String::new().into()
    }

    pub fn lower(&self) -> &str {
        &self.inner[..self.gap.start]
    }
    pub fn lower_mut(&mut self) -> &mut str {
        &mut self.inner[..self.gap.start]
    }

    pub fn upper(&self) -> &str {
        &self.inner[self.gap.end..]
    }
    pub fn upper_mut(&mut self) -> &mut str {
        &mut self.inner[self.gap.end..]
    }

    fn gap_mut(&mut self) -> &mut str {
        &mut self.inner[self.gap]
    }

    pub fn cursor(&self) -> usize {
        self.gap.start
    }

    pub fn len(&self) -> usize {
        self.inner.len() - self.gap_len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn gap_len(&self) -> usize {
        self.gap.end - self.gap.start
    }

    pub fn reserve(&mut self, new_gap_len: usize) {
        let additional = new_gap_len.saturating_sub(self.gap_len());
        if additional == 0 {
            return;
        }

        self.inner.reserve(additional);
        self.expand_gap();
    }

    pub fn expand_gap(&mut self) {
        // SAFETY: We will expand the gap with NULL bytes, keeping the existing bytes intact.
        // Since NULL is a valid UTF-8 character, this is safe.
        let data = unsafe { self.inner.as_mut_vec() };

        let additional_gap = data.capacity() - data.len();
        if additional_gap == 0 {
            return;
        }

        data.resize(data.capacity(), 0);
        data[self.gap.end..].rotate_right(additional_gap);
        self.gap.end += additional_gap;
    }

    pub fn insert_str(&mut self, src: &str) {
        self.reserve(src.len());

        // SAFETY: `src` is valid for `src.len()` bytes by definition. The destination has enough
        // bytes because we just reserved them. The strings cannot overlap because we have exclusive
        // access to `self`.
        unsafe {
            std::ptr::copy_nonoverlapping(src.as_ptr(), self.gap_mut().as_mut_ptr(), src.len());
        }

        self.gap.start += src.len();
    }

    pub fn delete(&mut self, n: usize) {
        self.gap.start = self.gap.start.saturating_sub(n);
    }

    pub fn clear(&mut self) {
        self.gap.start = 0;
        self.gap.end = self.inner.len();
    }

    pub fn move_cursor(&mut self, pos: usize) {
        assert!(self.inner.is_char_boundary(self.translate_idx(pos)));
        let gap_len = self.gap_len();
        let data = unsafe { self.inner.as_mut_vec() };

        if pos > self.gap.start {
            data[self.gap.start..pos + gap_len].rotate_left(gap_len);
        } else if pos < self.gap.start {
            data[pos..self.gap.end].rotate_right(gap_len);
        }

        self.gap.start = pos;
        self.gap.end = pos + gap_len;
    }

    fn translate_idx(&self, idx: usize) -> usize {
        if idx < self.gap.start {
            idx
        } else {
            idx + self.gap_len()
        }
    }

    pub fn get_byte(&self, idx: usize) -> Option<u8> {
        self.inner.as_bytes().get(self.translate_idx(idx)).copied()
    }

    fn range_to_indices(&self, range: impl RangeBounds<usize>) -> (usize, usize) {
        let start = match range.start_bound() {
            Bound::Included(&idx) => idx,
            Bound::Excluded(&idx) => idx + 1,
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&idx) => idx + 1,
            Bound::Excluded(&idx) => idx,
            Bound::Unbounded => self.len(),
        };
        (start, end)
    }

    pub fn char_indices(&self, range: impl RangeBounds<usize>) -> CharIndices<'_> {
        let (idx_start, idx_end) = self.range_to_indices(range);
        CharIndices {
            string: &self,
            idx_start,
            idx_end,
        }
    }

    pub fn chars(&self, range: impl RangeBounds<usize>) -> Chars<'_> {
        Chars {
            iter: self.char_indices(range),
        }
    }

    pub fn graphemes(&self, range: impl RangeBounds<usize>) -> Graphemes<'_> {
        let (idx_start, idx_end) = self.range_to_indices(range);
        Graphemes {
            string: self,
            cursor: GraphemeCursor::new(idx_start, self.len(), true),
            cursor_back: GraphemeCursor::new(idx_end, self.len(), true),
        }
    }
}

impl Default for GapString {
    fn default() -> Self {
        Self::new()
    }
}

impl From<String> for GapString {
    fn from(value: String) -> Self {
        Self {
            inner: value,
            gap: (0..0).into(),
        }
    }
}

impl fmt::Display for GapString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.lower())?;
        f.write_str(self.upper())?;
        Ok(())
    }
}

pub struct CharIndices<'a> {
    string: &'a GapString,
    idx_start: usize,
    idx_end: usize,
}

impl<'a> Iterator for CharIndices<'a> {
    type Item = (usize, char);

    fn next(&mut self) -> Option<Self::Item> {
        if self.idx_start >= self.idx_end {
            return None;
        }
        let idx = self.idx_start;
        let mut chars = self.string.inner[self.string.translate_idx(idx)..].chars();
        let pre_len = chars.as_str().len();
        let c = chars.next()?;
        self.idx_start += pre_len - chars.as_str().len();
        Some((idx, c))
    }
}

impl<'a> DoubleEndedIterator for CharIndices<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.idx_start >= self.idx_end {
            return None;
        }
        let idx = if self.idx_end == self.string.gap.start {
            self.string.gap.start
        } else {
            self.string.translate_idx(self.idx_end)
        };
        let mut chars = self.string.inner[..idx].chars();
        let pre_len = chars.as_str().len();
        let c = chars.next_back()?;
        self.idx_end -= pre_len - chars.as_str().len();
        Some((self.idx_end, c))
    }
}

pub struct Chars<'a> {
    iter: CharIndices<'a>,
}

impl<'a> Iterator for Chars<'a> {
    type Item = char;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|(_, c)| c)
    }
}

impl<'a> DoubleEndedIterator for Chars<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.iter.next_back().map(|(_, c)| c)
    }
}

pub struct Graphemes<'a> {
    string: &'a GapString,
    cursor: GraphemeCursor,
    cursor_back: GraphemeCursor,
}

impl<'a> Graphemes<'a> {
    pub fn read_next(&mut self, buf: &mut String) {
        let mut start = self.cursor.cur_cursor();
        if start >= self.cursor_back.cur_cursor() {
            return;
        }

        let (mut chunk, mut chunk_start) = if start < self.string.gap.start {
            (self.string.lower(), 0)
        } else {
            (self.string.upper(), self.string.gap.start)
        };

        loop {
            match self.cursor.next_boundary(chunk, chunk_start) {
                Ok(Some(boundary)) => {
                    buf.push_str(&chunk[start - chunk_start..boundary - chunk_start]);
                    break;
                }
                Ok(None) => break,
                Err(GraphemeIncomplete::PreContext(end)) => {
                    self.cursor.provide_context(&self.string.inner[..end], 0);
                }
                Err(GraphemeIncomplete::NextChunk) => {
                    buf.push_str(&chunk[start..]);
                    chunk = self.string.upper();
                    chunk_start = self.string.gap.start;
                    start = chunk_start;
                }
                Err(_) => unreachable!(),
            }
        }
    }

    pub fn read_next_back(&mut self, buf: &mut String) {
        let mut end = self.cursor_back.cur_cursor();
        if self.cursor.cur_cursor() >= end {
            return;
        }

        let (mut chunk, mut chunk_start) = if end <= self.string.gap.start {
            (self.string.lower(), 0)
        } else {
            (self.string.upper(), self.string.gap.start)
        };

        loop {
            match self.cursor_back.prev_boundary(chunk, chunk_start) {
                Ok(Some(boundary)) => {
                    buf.insert_str(0, &chunk[boundary - chunk_start..end - chunk_start]);
                    break;
                }
                Ok(None) => break,
                Err(GraphemeIncomplete::PreContext(end)) => {
                    self.cursor.provide_context(&self.string.inner[..end], 0);
                }
                Err(GraphemeIncomplete::PrevChunk) => {
                    buf.insert_str(0, &chunk[..end - chunk_start]);
                    chunk = self.string.lower();
                    chunk_start = 0;
                    end = self.string.gap.start;
                }
                Err(_) => unreachable!(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_and_move_cursor() {
        let mut s = GapString::from(String::from("hello"));
        assert_eq!(s.inner, "hello");

        s.inner.reserve_exact(5);
        s.expand_gap();
        assert_eq!(s.gap, (0..5).into());
        assert_eq!(s.inner, "\0\0\0\0\0hello");

        s.move_cursor(2);
        assert_eq!(s.gap, (2..7).into());
        assert_eq!(s.inner, "he\0\0\0\0\0llo");
    }

    #[test]
    fn insert() {
        let mut s = GapString::from(String::from("hello"));
        assert_eq!(s.to_string(), "hello");

        s.move_cursor(5);
        assert_eq!(s.to_string(), "hello");
        s.insert_str("world");
        assert_eq!(s.to_string(), "helloworld");
        s.insert_str("!");
        assert_eq!(s.to_string(), "helloworld!");
        s.move_cursor(5);
        assert_eq!(s.to_string(), "helloworld!");
        s.insert_str(", ");
        assert_eq!(s.to_string(), "hello, world!");
    }

    #[test]
    fn delete() {
        let mut s = GapString::from(String::from("hello, world!"));
        s.reserve(10);
        assert_eq!(s.to_string(), "hello, world!");

        s.move_cursor(13);
        assert_eq!(s.to_string(), "hello, world!");
        s.delete(1);
        assert_eq!(s.to_string(), "hello, world");
        s.move_cursor(7);
        assert_eq!(s.to_string(), "hello, world");
        s.delete(2);
        assert_eq!(s.to_string(), "helloworld");
    }

    #[test]
    fn get_byte() {
        let mut s = GapString::from(String::from("hello"));
        s.reserve(10);
        s.move_cursor(2);
        assert_eq!(s.get_byte(0), Some(b'h'));
        assert_eq!(s.get_byte(1), Some(b'e'));
        assert_eq!(s.get_byte(2), Some(b'l'));
        assert_eq!(s.get_byte(3), Some(b'l'));
        assert_eq!(s.get_byte(4), Some(b'o'));
    }

    #[test]
    fn chars() {
        let mut s = GapString::from(String::from("hello, world!"));
        s.reserve(10);
        s.move_cursor(5);
        assert_eq!(s.chars(..).collect::<String>(), "hello, world!");
        assert_eq!(s.chars(7..).collect::<String>(), "world!");
        assert_eq!(s.chars(13..).collect::<String>(), "");
        assert_eq!(s.chars(..13).rev().collect::<String>(), "!dlrow ,olleh");
        assert_eq!(s.chars(..=4).rev().collect::<String>(), "olleh");
    }

    #[test]
    fn graphemes() {
        let mut s = GapString::from(String::from("hello 👨‍👩‍👧‍👦👩🏽‍💻🇧🇪❤️ world!"));
        s.reserve(10);
        s.move_cursor(42);

        let mut grapheme = String::new();

        let mut graphemes = s.graphemes(6..60);
        graphemes.read_next(&mut grapheme);
        assert_eq!(grapheme, "👨‍👩‍👧‍👦");
        grapheme.clear();
        graphemes.read_next(&mut grapheme);
        assert_eq!(grapheme, "👩🏽‍💻");
        grapheme.clear();
        graphemes.read_next_back(&mut grapheme);
        assert_eq!(grapheme, "❤️");
        grapheme.clear();
        graphemes.read_next_back(&mut grapheme);
        assert_eq!(grapheme, "🇧🇪");
        grapheme.clear();
        graphemes.read_next(&mut grapheme);
        assert_eq!(grapheme, "");
        grapheme.clear();
        graphemes.read_next_back(&mut grapheme);
        assert_eq!(grapheme, "");
    }
}
