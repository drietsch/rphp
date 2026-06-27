//! Source files and line-index mapping.
//!
//! PHP source is a byte sequence, never assumed UTF-8, so sources are stored as
//! `Vec<u8>`. A [`SourceMap`] owns all files in a compilation and hands out
//! [`FileId`]s; [`SourceFile`] maps byte offsets to 1-based (line, column).
#![forbid(unsafe_code)]

use rphp_span::FileId;

pub struct SourceFile {
    pub id: FileId,
    pub name: String,
    pub src: Vec<u8>,
    /// Byte offset of the start of each line (line 0 starts at offset 0).
    line_starts: Vec<u32>,
}

impl SourceFile {
    fn new(id: FileId, name: String, src: Vec<u8>) -> Self {
        let mut line_starts = vec![0u32];
        for (i, &b) in src.iter().enumerate() {
            if b == b'\n' {
                line_starts.push((i + 1) as u32);
            }
        }
        Self { id, name, src, line_starts }
    }

    /// 1-based (line, column) for a byte offset. Column counts bytes.
    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        let line = match self.line_starts.binary_search(&offset) {
            Ok(l) => l,
            Err(l) => l - 1,
        };
        let col = offset - self.line_starts[line] + 1;
        (line as u32 + 1, col)
    }

    /// The raw bytes of a 1-based line, without the trailing newline.
    pub fn line_text(&self, line: u32) -> &[u8] {
        let idx = (line.saturating_sub(1)) as usize;
        let start = self.line_starts.get(idx).copied().unwrap_or(0) as usize;
        let end = self
            .line_starts
            .get(idx + 1)
            .map(|&e| e as usize - 1)
            .unwrap_or(self.src.len());
        &self.src[start..end.min(self.src.len())]
    }
}

#[derive(Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, name: impl Into<String>, src: impl Into<Vec<u8>>) -> FileId {
        let id = FileId(self.files.len() as u32);
        self.files.push(SourceFile::new(id, name.into(), src.into()));
        id
    }

    pub fn get(&self, id: FileId) -> &SourceFile {
        &self.files[id.0 as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_mapping() {
        let mut sm = SourceMap::new();
        let f = sm.add("t.php", &b"ab\ncde\nf"[..]);
        let file = sm.get(f);
        assert_eq!(file.line_col(0), (1, 1));
        assert_eq!(file.line_col(1), (1, 2));
        assert_eq!(file.line_col(3), (2, 1)); // 'c'
        assert_eq!(file.line_col(7), (3, 1)); // 'f'
        assert_eq!(file.line_text(2), b"cde");
    }
}
