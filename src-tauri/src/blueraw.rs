//! EVE settings files (`core_user_*.dat`, `core_char_*.dat`) read without losing a byte, so one
//! value can be changed and the file written back exactly as EVE wrote it otherwise (20 Nov).
//!
//! `blue.rs` reads these files into plain values for showing and comparing; that's lossy (the
//! same number can be stored several ways). Here every value keeps its own opcode byte, its
//! length bytes and its payload, so `write(parse(file)) == file` byte for byte. This is the
//! ground under seats and templates (client-setup-plan.md: "Writing .dat files: re-serialise
//! and check the result reads back identically before any value edits").
//!
//! Nothing here touches a file on disk: it turns bytes into a tree and a tree into bytes.

/// A whole settings file: the value tree and the shared-object slot table at the end.
#[derive(Debug, Clone, PartialEq)]
pub struct File {
    pub root: Node,
    pub tail: Vec<u32>,
}

/// One value as stored: opcode byte (0x40 = "also kept as a shared object"), its length
/// bytes as written, and what follows.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub op: u8,
    pub head: Vec<u8>,
    pub body: Body,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// Bytes after the opcode and length: numbers, text, a REF's slot (in `head`).
    Raw(Vec<u8>),
    /// Tuples, lists and two-part objects.
    Items(Vec<Node>),
    /// Dicts, in file order; each entry is stored value first, then key.
    Dict(Vec<(Node, Node)>),
    /// Objects 0x22 / 0x23: parts up to the first 0x2d, then key/value parts up to the second.
    Ext(Vec<Node>, Vec<Node>),
}

const MAX_DEPTH: usize = 200;
const MARK: u8 = 0x2d;

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
    end: usize,
    shared: usize,
}

impl<'a> Parser<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.at + n > self.end {
            return Err(format!("ran past the end at byte {}", self.at));
        }
        let out = &self.bytes[self.at..self.at + n];
        self.at += n;
        Ok(out)
    }

    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn peek(&self) -> Result<u8, String> {
        if self.at < self.end { Ok(self.bytes[self.at]) } else { Err("ran past the end".into()) }
    }

    /// A length as written (one byte, or 0xff and four) and its value.
    fn size(&mut self) -> Result<(Vec<u8>, usize), String> {
        let n = self.byte()?;
        if n == 0xff {
            let b = self.take(4)?;
            let mut head = vec![0xff];
            head.extend_from_slice(b);
            return Ok((head, u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize));
        }
        Ok((vec![n], n as usize))
    }

    fn items(&mut self, n: usize, depth: usize) -> Result<Vec<Node>, String> {
        let mut out = Vec::with_capacity(n.min(4096));
        for _ in 0..n {
            out.push(self.node(depth + 1)?);
        }
        Ok(out)
    }

    fn node(&mut self, depth: usize) -> Result<Node, String> {
        if depth > MAX_DEPTH {
            return Err("nested too deep".into());
        }
        let at = self.at;
        let op = self.byte()?;
        if op & 0x80 != 0 {
            return Err(format!("unknown opcode {op:#04x} at byte {at}"));
        }
        if op & 0x40 != 0 {
            self.shared += 1;
        }
        let raw = |p: &mut Self, n: usize| -> Result<Body, String> { Ok(Body::Raw(p.take(n)?.to_vec())) };
        let (head, body) = match op & 0x3f {
            0x01 | 0x07 | 0x08 | 0x09 | 0x0b | 0x0e | 0x28 | 0x1f | 0x20 | 0x24 | 0x26 => (vec![], Body::Raw(vec![])),
            0x03 | 0x0a => (vec![], raw(self, 8)?),
            0x04 => (vec![], raw(self, 4)?),
            0x05 | 0x29 => (vec![], raw(self, 2)?),
            0x06 | 0x0f | 0x11 => (vec![], raw(self, 1)?),
            0x02 | 0x10 | 0x13 | 0x2e => {
                let (head, n) = self.size()?;
                (head, raw(self, n)?)
            }
            0x12 => {
                let (head, n) = self.size()?;
                (head, raw(self, n.checked_mul(2).ok_or("bad length")?)?)
            }
            0x2f => {
                let n = self.byte()?;
                (vec![n], raw(self, n as usize)?)
            }
            0x1b => {
                let (head, _) = self.size()?;
                (head, Body::Raw(vec![]))
            }
            0x14 | 0x15 => {
                let (head, n) = self.size()?;
                (head, Body::Items(self.items(n, depth)?))
            }
            0x25 | 0x27 => (vec![], Body::Items(self.items(1, depth)?)),
            0x2c | 0x17 => (vec![], Body::Items(self.items(2, depth)?)),
            0x16 => {
                let (head, n) = self.size()?;
                let mut entries = Vec::with_capacity(n.min(4096));
                for _ in 0..n {
                    let value = self.node(depth + 1)?;
                    let key = self.node(depth + 1)?;
                    entries.push((value, key));
                }
                (head, Body::Dict(entries))
            }
            0x22 | 0x23 => {
                let mut first = vec![self.node(depth + 1)?];
                while self.peek()? != MARK {
                    first.push(self.node(depth + 1)?);
                }
                self.at += 1;
                let mut pairs = Vec::new();
                while self.peek()? != MARK {
                    pairs.push(self.node(depth + 1)?);
                    pairs.push(self.node(depth + 1)?);
                }
                self.at += 1;
                (vec![], Body::Ext(first, pairs))
            }
            other => return Err(format!("unknown opcode {other:#04x} at byte {at}")),
        };
        Ok(Node { op, head, body })
    }
}

/// Read a whole file, keeping every byte. Fails (never panics) on anything it doesn't know.
pub fn parse(bytes: &[u8]) -> Result<File, String> {
    if bytes.len() < 6 || bytes[0] != 0x7e {
        return Err("not an EVE settings file".into());
    }
    let count = u32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]) as usize;
    let tail_len = count.checked_mul(4).ok_or("bad shared count")?;
    if tail_len + 5 > bytes.len() {
        return Err("bad shared count".into());
    }
    let end = bytes.len() - tail_len;
    let tail = bytes[end..].chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let mut parser = Parser { bytes, at: 5, end, shared: 0 };
    let root = parser.node(0)?;
    if parser.at != end {
        return Err(format!("{} bytes left over", end - parser.at));
    }
    if parser.shared != count {
        return Err(format!("found {} shared objects, the file says {count}", parser.shared));
    }
    Ok(File { root, tail })
}

impl File {
    /// The file's bytes again (identical to what was parsed, unless something was changed).
    pub fn write(&self) -> Vec<u8> {
        let mut out = vec![0x7e];
        out.extend((self.tail.len() as u32).to_le_bytes());
        self.root.write_to(&mut out);
        for slot in &self.tail {
            out.extend(slot.to_le_bytes());
        }
        out
    }
}

impl Node {
    fn write_to(&self, out: &mut Vec<u8>) {
        out.push(self.op);
        out.extend(&self.head);
        match &self.body {
            Body::Raw(bytes) => out.extend(bytes),
            Body::Items(items) => items.iter().for_each(|n| n.write_to(out)),
            Body::Dict(entries) => {
                for (value, key) in entries {
                    value.write_to(out);
                    key.write_to(out);
                }
            }
            Body::Ext(first, pairs) => {
                first.iter().for_each(|n| n.write_to(out));
                out.push(MARK);
                pairs.iter().for_each(|n| n.write_to(out));
                out.push(MARK);
            }
        }
    }

    /// Whether this value is also kept as a shared object (others may point at it).
    pub fn is_shared(&self) -> bool {
        self.op & 0x40 != 0
    }

    /// Whether this value or anything inside it is shared (changing it would change what
    /// other places pointing at it see, or renumber the slots).
    pub fn holds_shared(&self) -> bool {
        self.is_shared()
            || match &self.body {
                Body::Raw(_) => false,
                Body::Items(items) => items.iter().any(Node::holds_shared),
                Body::Dict(entries) => entries.iter().any(|(v, k)| v.holds_shared() || k.holds_shared()),
                Body::Ext(a, b) => a.iter().chain(b).any(Node::holds_shared),
            }
    }

    /// Whether this is a REF to a shared object.
    pub fn is_ref(&self) -> bool {
        self.op & 0x3f == 0x1b
    }

    /// A key's text (plain, wide or single-character strings), when it is one.
    pub fn text(&self) -> Option<String> {
        let Body::Raw(bytes) = &self.body else { return None };
        match self.op & 0x3f {
            0x10 | 0x13 | 0x2e | 0x0f => Some(String::from_utf8_lossy(bytes).into_owned()),
            0x0e | 0x28 => Some(String::new()),
            0x12 | 0x29 => Some(String::from_utf16_lossy(&bytes.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())),
            _ => None,
        }
    }

    /// The entry under a text key of a dict (the first; keys stored as REFs aren't followed).
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Node> {
        let Body::Dict(entries) = &mut self.body else { return None };
        entries.iter_mut().find(|(_, k)| k.text().as_deref() == Some(key)).map(|(v, _)| v)
    }

    pub fn get(&self, key: &str) -> Option<&Node> {
        let Body::Dict(entries) = &self.body else { return None };
        entries.iter().find(|(_, k)| k.text().as_deref() == Some(key)).map(|(v, _)| v)
    }

    /// Follow a path of text keys through dicts.
    pub fn at_mut(&mut self, path: &[&str]) -> Option<&mut Node> {
        path.iter().try_fold(self, |node, key| node.get_mut(key))
    }

    /// The (file time, value) pair EVE stores most settings as: the value part.
    pub fn stamped_value_mut(&mut self) -> Option<&mut Node> {
        let kind = self.op & 0x3f;
        match &mut self.body {
            Body::Items(pair) if matches!(kind, 0x2c | 0x14) && pair.len() == 2 => pair.get_mut(1),
            _ => None,
        }
    }

    /// A number, as EVE writes the smallest form (never shared).
    pub fn int(value: i64) -> Node {
        let (op, raw) = match value {
            -1 => (0x07, vec![]),
            0 => (0x08, vec![]),
            1 => (0x09, vec![]),
            v if i8::try_from(v).is_ok() => (0x06, (v as i8).to_le_bytes().to_vec()),
            v if i16::try_from(v).is_ok() => (0x05, (v as i16).to_le_bytes().to_vec()),
            v if i32::try_from(v).is_ok() => (0x04, (v as i32).to_le_bytes().to_vec()),
            v => (0x03, v.to_le_bytes().to_vec()),
        };
        Node { op, head: vec![], body: Body::Raw(raw) }
    }

    pub fn float(value: f64) -> Node {
        if value == 0.0 {
            Node { op: 0x0b, head: vec![], body: Body::Raw(vec![]) }
        } else {
            Node { op: 0x0a, head: vec![], body: Body::Raw(value.to_le_bytes().to_vec()) }
        }
    }

    pub fn bool(value: bool) -> Node {
        Node { op: if value { 0x1f } else { 0x20 }, head: vec![], body: Body::Raw(vec![]) }
    }
}

/// Change one value inside a file without touching anything else. Refuses when the old or the
/// new value is (or holds) a shared object, since other places may point at it.
pub fn replace(file: &mut File, path: &[&str], new: Node) -> Result<(), String> {
    if new.holds_shared() {
        return Err("the new value can't be a shared object".into());
    }
    let node = file.root.at_mut(path).ok_or_else(|| format!("no {} in this file", path.join(" → ")))?;
    let stamped = node.stamped_value_mut().is_some();
    let target = if stamped { node.stamped_value_mut().expect("checked") } else { node };
    if target.holds_shared() || target.is_ref() {
        return Err(format!("{} is shared with other settings; not changed", path.join(" → ")));
    }
    *target = new;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// header (1 shared), dict of 2: ("audio": (stamp, 0.5)), ("ui": {"turretsEnabled": (stamp, 1), "audio": True by REF}).
    fn sample() -> Vec<u8> {
        let stamp = 134_394_328_018_117_906i64.to_le_bytes();
        let mut b = vec![0x7e, 1, 0, 0, 0];
        b.extend([0x16, 2]);
        b.extend([0x2c, 0x2f, 8]);
        b.extend(stamp);
        b.push(0x0a);
        b.extend(0.5f64.to_le_bytes());
        b.extend([0x13 | 0x40, 5]);
        b.extend(b"audio");
        b.extend([0x16, 2]);
        b.extend([0x2c, 0x2f, 8]);
        b.extend(stamp);
        b.push(0x09);
        b.extend([0x13, 14]);
        b.extend(b"turretsEnabled");
        b.push(0x1f);
        b.extend([0x1b, 1]);
        b.extend([0x13, 2]);
        b.extend(b"ui");
        b.extend(1u32.to_le_bytes());
        b
    }

    #[test]
    fn writes_back_byte_for_byte() {
        let bytes = sample();
        assert_eq!(parse(&bytes).unwrap().write(), bytes);
    }

    #[test]
    fn changes_one_stamped_value_and_keeps_the_rest() {
        let bytes = sample();
        let mut file = parse(&bytes).unwrap();
        replace(&mut file, &["ui", "turretsEnabled"], Node::int(0)).unwrap();
        let out = file.write();
        assert_eq!(out.len(), bytes.len());
        assert_eq!(out.iter().zip(&bytes).filter(|(a, b)| a != b).count(), 1);
        // A value stored under a shared key is still its own value.
        assert!(replace(&mut file, &["audio"], Node::float(0.7)).is_ok());
        assert!(replace(&mut file, &["nope"], Node::int(3)).is_err());
    }

    #[test]
    fn refuses_to_touch_shared_values() {
        let mut file = parse(&sample()).unwrap();
        // ui → audio is a REF to the shared "audio" key.
        assert!(replace(&mut file, &["ui", "audio"], Node::bool(false)).is_err());
        let shared = Node { op: 0x09 | 0x40, head: vec![], body: Body::Raw(vec![]) };
        assert!(replace(&mut file, &["ui", "turretsEnabled"], shared).is_err());
    }

    #[test]
    fn numbers_take_evess_smallest_form() {
        for (v, op) in [(-1, 0x07), (0, 0x08), (1, 0x09), (100, 0x06), (-300, 0x05), (70_000, 0x04), (1 << 40, 0x03)] {
            assert_eq!(Node::int(v).op, op, "{v}");
        }
    }

    #[test]
    fn broken_files_fail_without_panicking() {
        let good = sample();
        for cut in 0..good.len() {
            let _ = parse(&good[..cut]);
        }
        assert!(parse(b"nope").is_err());
    }

    /// Every real settings file in $BLUE_FILES (a folder) writes back byte for byte.
    /// Run with: BLUE_FILES=/path cargo test --lib blueraw -- --ignored
    #[test]
    #[ignore]
    fn real_files_write_back_byte_for_byte() {
        let dir = std::env::var("BLUE_FILES").expect("BLUE_FILES");
        let mut checked = 0;
        for entry in walk(std::path::Path::new(&dir)) {
            let bytes = std::fs::read(&entry).unwrap();
            let file = parse(&bytes).unwrap_or_else(|e| panic!("{}: {e}", entry.display()));
            assert_eq!(file.write(), bytes, "{}", entry.display());
            checked += 1;
        }
        assert!(checked > 0);
        eprintln!("{checked} files identical");
    }

    fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk(&path));
            } else if path.extension().is_some_and(|e| e == "dat") {
                out.push(path);
            }
        }
        out
    }
}
