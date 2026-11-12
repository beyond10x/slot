//! Replaces the server workspace path with the client path in streamed
//! output, line by line, so a path is never split across chunks.

/// Line-buffered path rewriter.
#[derive(Debug, Default)]
pub struct Rewriter {
    from: Vec<u8>,
    to: Vec<u8>,
    buf: Vec<u8>,
}

const MAX_LINE: usize = 64 << 10;

impl Rewriter {
    /// A rewriter that passes bytes through unchanged when `from` is empty.
    pub fn new(from: &str, to: &str) -> Self {
        Self {
            from: from.as_bytes().to_vec(),
            to: to.as_bytes().to_vec(),
            buf: Vec::new(),
        }
    }

    /// Feeds bytes; returns what can be emitted now.
    pub fn push(&mut self, p: &[u8]) -> Vec<u8> {
        if self.from.is_empty() {
            return p.to_vec();
        }
        self.buf.extend_from_slice(p);
        let cut = match self.buf.iter().rposition(|&b| b == b'\n') {
            Some(i) => i + 1,
            None if self.buf.len() >= MAX_LINE => self.buf.len(),
            None => return Vec::new(),
        };
        let rest = self.buf.split_off(cut);
        let out = replace(&self.buf, &self.from, &self.to);
        self.buf = rest;
        out
    }

    /// Flushes a trailing partial line.
    pub fn finish(&mut self) -> Vec<u8> {
        let out = replace(&self.buf, &self.from, &self.to);
        self.buf.clear();
        out
    }
}

fn replace(hay: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    if from.is_empty() {
        return hay.to_vec();
    }
    let mut out = Vec::with_capacity(hay.len());
    let mut i = 0;
    while i < hay.len() {
        if hay[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_split_across_writes() {
        let mut r = Rewriter::new("/srv/ws/abc", "/client/repo");
        let mut out = Vec::new();
        for p in [
            "error at /srv/w",
            "s/abc/src/lib.rs:3\nnext /srv/ws/",
            "abc/x",
        ] {
            out.extend(r.push(p.as_bytes()));
        }
        out.extend(r.finish());
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "error at /client/repo/src/lib.rs:3\nnext /client/repo/x"
        );
    }

    #[test]
    fn empty_from_passes_through() {
        let mut r = Rewriter::new("", "");
        assert_eq!(r.push(b"partial"), b"partial");
    }
}
