//! Strict syntax check before serde-json-core skips forward-compatible fields.
//! Its IgnoredAny intentionally skips unknown primitive tokens without validating
//! their JSON spelling. Bound nesting as well as bytes before deserialization.
const MAX_DEPTH: u8 = 12;
pub fn valid(bytes: &[u8]) -> bool {
    if core::str::from_utf8(bytes).is_err() {
        return false;
    }
    let mut p = Parser { bytes, pos: 0 };
    p.value(0) && {
        p.space();
        p.pos == bytes.len()
    }
}
struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }
    fn take(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.pos += 1;
        }
    }
    fn literal(&mut self, s: &[u8]) -> bool {
        if self.bytes.get(self.pos..self.pos + s.len()) == Some(s) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }
    fn hex4(&mut self) -> Option<u16> {
        let mut n = 0;
        for _ in 0..4 {
            let b = self.peek()?;
            let digit = match b {
                b'0'..=b'9' => b - b'0',
                b'a'..=b'f' => b - b'a' + 10,
                b'A'..=b'F' => b - b'A' + 10,
                _ => return None,
            };
            self.pos += 1;
            n = n * 16 + u16::from(digit);
        }
        Some(n)
    }
    fn string(&mut self) -> bool {
        if !self.take(b'"') {
            return false;
        }
        while let Some(b) = self.peek() {
            self.pos += 1;
            match b {
                b'"' => return true,
                0..=31 => return false,
                b'\\' => {
                    let Some(escape) = self.peek() else {
                        return false;
                    };
                    self.pos += 1;
                    match escape {
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {}
                        b'u' => {
                            let Some(cp) = self.hex4() else {
                                return false;
                            };
                            if (0xd800..=0xdbff).contains(&cp) {
                                if !self.literal(b"\\u")
                                    || !self
                                        .hex4()
                                        .is_some_and(|low| (0xdc00..=0xdfff).contains(&low))
                                {
                                    return false;
                                }
                            } else if (0xdc00..=0xdfff).contains(&cp) {
                                return false;
                            }
                        }
                        _ => return false,
                    }
                }
                _ => {}
            }
        }
        false
    }
    fn digits(&mut self) -> bool {
        let start = self.pos;
        while self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.pos += 1;
        }
        self.pos != start
    }
    fn number(&mut self) -> bool {
        self.take(b'-');
        if !self.take(b'0') && !self.digits() {
            return false;
        }
        if self.take(b'.') && !self.digits() {
            return false;
        }
        if self.take(b'e') || self.take(b'E') {
            if !self.take(b'+') {
                self.take(b'-');
            }
            if !self.digits() {
                return false;
            }
        }
        true
    }
    fn value(&mut self, depth: u8) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        self.space();
        match self.peek() {
            Some(b'"') => self.string(),
            Some(b't') => self.literal(b"true"),
            Some(b'f') => self.literal(b"false"),
            Some(b'n') => self.literal(b"null"),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(b'{' | b'[') => {
                let object = self.take(b'{');
                if !object {
                    self.pos += 1;
                }
                let end = if object { b'}' } else { b']' };
                self.space();
                if self.take(end) {
                    return true;
                }
                loop {
                    self.space();
                    if object {
                        if !self.string() {
                            return false;
                        }
                        self.space();
                        if !self.take(b':') {
                            return false;
                        }
                    }
                    if !self.value(depth + 1) {
                        return false;
                    }
                    self.space();
                    if self.take(end) {
                        return true;
                    }
                    if !self.take(b',') {
                        return false;
                    }
                }
            }
            _ => false,
        }
    }
}
