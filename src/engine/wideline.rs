// English comments: UTF-16 line reader shared by index and search workers.
// Carry-safe: no bytes lost between lines (parity with both former copies).

/// Wide (UTF-16) line reader with cross-chunk carry.
pub struct WideLineReader<R: std::io::Read> {
    inner: std::io::BufReader<R>,
    pending: Vec<u8>,
    le: bool,
}

impl<R: std::io::Read> WideLineReader<R> {
    pub fn new(inner: std::io::BufReader<R>, le: bool) -> Self {
        Self {
            inner,
            pending: Vec::new(),
            le,
        }
    }

    /// Mutable access to the inner reader (for caller seeks).
    pub fn inner_mut(&mut self) -> &mut std::io::BufReader<R> {
        &mut self.inner
    }

    /// Seek inner reader to absolute offset and drop any carried bytes.
    pub fn seek_start(&mut self, pos: u64) -> std::io::Result<()>
    where
        R: std::io::Seek,
    {
        use std::io::Seek as _;
        self.inner.seek(std::io::SeekFrom::Start(pos))?;
        self.pending.clear();
        Ok(())
    }

    /// Drop carried bytes without seeking (e.g. after a cancel/abort).
    pub fn clear_pending(&mut self) {
        self.pending.clear();
    }

    /// Read one line (including the newline unit) into `buf`.
    /// Returns bytes consumed; 0 = EOF with no data. A final line without
    /// a trailing newline is still returned (buf non-empty).
    pub fn read_line(&mut self, buf: &mut Vec<u8>) -> u64 {
        use std::io::Read as _;
        let (n1, n2) = if self.le {
            (0x0Au8, 0x00u8)
        } else {
            (0x00u8, 0x0Au8)
        };
        let mut consumed: u64 = 0;
        loop {
            if !self.pending.is_empty() {
                let mut i = 0usize;
                while i + 1 < self.pending.len() {
                    if self.pending[i] == n1 && self.pending[i + 1] == n2 {
                        buf.extend_from_slice(&self.pending[..i + 2]);
                        consumed += (i + 2) as u64;
                        self.pending.drain(..i + 2);
                        return consumed;
                    }
                    i += 2;
                }
                // No newline: all pending minus a possible half unit belongs here.
                let keep_to = self.pending.len().saturating_sub(1).min(self.pending.len());
                buf.extend_from_slice(&self.pending[..keep_to]);
                consumed += keep_to as u64;
                self.pending.drain(..keep_to);
            }
            let mut chunk = [0u8; 64 * 1024];
            match self.inner.read(&mut chunk) {
                Ok(0) => {
                    if self.pending.is_empty() && buf.is_empty() {
                        return 0;
                    }
                    if !self.pending.is_empty() {
                        buf.extend_from_slice(&self.pending);
                        consumed += self.pending.len() as u64;
                        self.pending.clear();
                    }
                    return consumed;
                }
                Ok(n) => {
                    self.pending.extend_from_slice(&chunk[..n]);
                }
                Err(_) => {
                    if self.pending.is_empty() && buf.is_empty() {
                        return 0;
                    }
                    buf.extend_from_slice(&self.pending);
                    consumed += self.pending.len() as u64;
                    self.pending.clear();
                    return consumed;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_utf16le_lines_across_chunk_boundary() {
        // "a\nb\n" as UTF-16 LE: a=0x61,0x00 n=0x0A,0x00 …
        let mut bytes = Vec::new();
        for ch in ['a', '\n', 'b', '\n'] {
            let u = ch as u16;
            bytes.push((u & 0xFF) as u8);
            bytes.push((u >> 8) as u8);
        }
        let mut r = WideLineReader::new(std::io::BufReader::with_capacity(4, &bytes[..]), true);
        let mut buf = Vec::new();
        assert!(r.read_line(&mut buf) > 0);
        assert_eq!(buf, b"a\x00\n\x00");
        buf.clear();
        assert!(r.read_line(&mut buf) > 0);
        assert_eq!(buf, b"b\x00\n\x00");
        buf.clear();
        assert_eq!(r.read_line(&mut buf), 0);
    }

    #[test]
    fn returns_final_line_without_newline() {
        let bytes = [0x61u8, 0x00]; // "a" LE, no newline
        let mut r = WideLineReader::new(std::io::BufReader::with_capacity(8, &bytes[..]), true);
        let mut buf = Vec::new();
        assert!(r.read_line(&mut buf) > 0);
        assert_eq!(buf, bytes);
        buf.clear();
        assert_eq!(r.read_line(&mut buf), 0);
    }
}
