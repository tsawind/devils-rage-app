//! Patch 73: read EVE's settings files (`core_user_*.dat`, `core_char_*.dat`).
//!
//! They are CCP's "blue marshal": a `0x7e` header with how many shared objects there are, one
//! value (a dict of sections), then that many 4-byte slot numbers at the very end. Each value is
//! one opcode byte (`0x40` set = "also remember this as shared") followed by its data; dicts
//! store each entry value first, then key. A `REF` points back at a shared object by slot.
//!
//! Most settings are `(file time, value)` pairs; [`unstamped`] drops the time. This module only
//! reads; nothing here writes a file. Checked against Darkness Holder's account and character
//! files (17 Nov): every byte read, every shared object accounted for.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Blue {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Blue>),
    Tuple(Vec<Blue>),
    Dict(Vec<(Blue, Blue)>),
    /// A class name, an instance or a pickled object: kept, only ever compared.
    Object(Vec<Blue>),
}

impl Blue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Blue::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The entry under a string key of a dict.
    pub fn get(&self, key: &str) -> Option<&Blue> {
        match self {
            Blue::Dict(entries) => entries.iter().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn entries(&self) -> &[(Blue, Blue)] {
        match self {
            Blue::Dict(entries) => entries,
            _ => &[],
        }
    }

    /// A short, stable text for this value (dict keys sorted), for showing and comparing.
    pub fn text(&self) -> String {
        match self {
            Blue::None => "none".into(),
            Blue::Bool(b) => if *b { "on".into() } else { "off".into() },
            Blue::Int(i) => i.to_string(),
            Blue::Float(f) => {
                let r = (f * 1000.0).round() / 1000.0;
                if r.fract() == 0.0 { format!("{r:.1}") } else { r.to_string() }
            }
            Blue::Str(s) => s.clone(),
            Blue::List(items) | Blue::Tuple(items) | Blue::Object(items) => {
                format!("({})", items.iter().map(Blue::text).collect::<Vec<_>>().join(", "))
            }
            Blue::Dict(entries) => {
                let mut parts: Vec<String> = entries.iter().map(|(k, v)| format!("{}: {}", k.text(), v.text())).collect();
                parts.sort();
                format!("{{{}}}", parts.join(", "))
            }
        }
    }

    /// A fingerprint of the value (FNV-1a of [`Blue::text`]), to tell equal setups apart quickly.
    pub fn fingerprint(&self) -> String {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in self.text().bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{:08x}", hash >> 32)
    }
}

/// `(file time, value)` -> value. EVE stamps each setting with when it was set.
pub fn unstamped(value: &Blue) -> &Blue {
    match value {
        Blue::Tuple(pair) if pair.len() == 2 && matches!(pair[0], Blue::Int(t) if t > 100_000_000_000_000_000) => &pair[1],
        _ => value,
    }
}

const MAX_DEPTH: usize = 200;

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    end: usize,
    tail: Vec<u32>,
    shared: usize,
    slots: HashMap<u32, Blue>,
}

impl<'a> Reader<'a> {
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
        self.bytes.get(self.at).copied().filter(|_| self.at < self.end).ok_or_else(|| "ran past the end".to_string())
    }

    /// A length: one byte, or 0xff and then four.
    fn size(&mut self) -> Result<usize, String> {
        let n = self.byte()?;
        if n == 0xff {
            let b = self.take(4)?;
            return Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
        }
        Ok(n as usize)
    }

    fn text(&mut self, n: usize) -> Result<String, String> {
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }

    fn value(&mut self, depth: usize) -> Result<Blue, String> {
        if depth > MAX_DEPTH {
            return Err("nested too deep".into());
        }
        let at = self.at;
        let op = self.byte()?;
        if op & 0x80 != 0 {
            return Err(format!("unknown opcode {op:#04x} at byte {at}"));
        }
        let value = self.body(op & 0x3f, depth, at)?;
        if op & 0x40 != 0 {
            let slot = *self.tail.get(self.shared).ok_or("more shared objects than the file says")?;
            self.shared += 1;
            self.slots.insert(slot, value.clone());
        }
        Ok(value)
    }

    fn items(&mut self, n: usize, depth: usize) -> Result<Vec<Blue>, String> {
        let mut out = Vec::with_capacity(n.min(4096));
        for _ in 0..n {
            out.push(self.value(depth + 1)?);
        }
        Ok(out)
    }

    fn body(&mut self, op: u8, depth: usize, at: usize) -> Result<Blue, String> {
        Ok(match op {
            0x01 => Blue::None,
            0x02 => {
                let n = self.size()?;
                Blue::Object(vec![Blue::Str(self.text(n)?)])
            }
            0x03 => Blue::Int(i64::from_le_bytes(self.take(8)?.try_into().unwrap())),
            0x04 => Blue::Int(i32::from_le_bytes(self.take(4)?.try_into().unwrap()) as i64),
            0x05 => Blue::Int(i16::from_le_bytes(self.take(2)?.try_into().unwrap()) as i64),
            0x06 => Blue::Int(self.byte()? as i8 as i64),
            0x07 => Blue::Int(-1),
            0x08 => Blue::Int(0),
            0x09 => Blue::Int(1),
            0x0a => Blue::Float(f64::from_le_bytes(self.take(8)?.try_into().unwrap())),
            0x0b => Blue::Float(0.0),
            0x0e | 0x28 => Blue::Str(String::new()),
            0x0f => Blue::Str(self.text(1)?),
            0x10 | 0x13 | 0x2e => {
                let n = self.size()?;
                Blue::Str(self.text(n)?)
            }
            0x11 => Blue::Str(format!("#string{}", self.byte()?)),
            0x12 => {
                let n = self.size()?;
                let raw = self.take(n * 2)?;
                let units: Vec<u16> = raw.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                Blue::Str(String::from_utf16_lossy(&units))
            }
            0x29 => {
                let raw = self.take(2)?;
                Blue::Str(String::from_utf16_lossy(&[u16::from_le_bytes([raw[0], raw[1]])]))
            }
            0x14 => {
                let n = self.size()?;
                Blue::Tuple(self.items(n, depth)?)
            }
            0x15 => {
                let n = self.size()?;
                Blue::List(self.items(n, depth)?)
            }
            0x24 => Blue::Tuple(Vec::new()),
            0x25 => Blue::Tuple(self.items(1, depth)?),
            0x2c => Blue::Tuple(self.items(2, depth)?),
            0x26 => Blue::List(Vec::new()),
            0x27 => Blue::List(self.items(1, depth)?),
            0x16 => {
                let n = self.size()?;
                let mut entries = Vec::with_capacity(n.min(4096));
                for _ in 0..n {
                    let value = self.value(depth + 1)?;
                    let key = self.value(depth + 1)?;
                    entries.push((key, value));
                }
                Blue::Dict(entries)
            }
            0x1b => {
                let slot = self.size()? as u32;
                self.slots.get(&slot).cloned().ok_or_else(|| format!("points at shared object {slot} before it exists"))?
            }
            0x1f => Blue::Bool(true),
            0x20 => Blue::Bool(false),
            0x2f => {
                let n = self.byte()? as usize;
                let raw = self.take(n)?;
                if n <= 8 {
                    let mut buf = [if raw.last().map_or(false, |b| b & 0x80 != 0) { 0xff } else { 0 }; 8];
                    buf[..n].copy_from_slice(raw);
                    Blue::Int(i64::from_le_bytes(buf))
                } else {
                    Blue::Str(raw.iter().map(|b| format!("{b:02x}")).collect())
                }
            }
            0x17 => Blue::Object(self.items(2, depth)?),
            0x22 | 0x23 => {
                let mut parts = vec![self.value(depth + 1)?];
                while self.peek()? != 0x2d {
                    parts.push(self.value(depth + 1)?);
                }
                self.at += 1;
                while self.peek()? != 0x2d {
                    parts.push(self.value(depth + 1)?);
                    parts.push(self.value(depth + 1)?);
                }
                self.at += 1;
                Blue::Object(parts)
            }
            _ => return Err(format!("unknown opcode {op:#04x} at byte {at}")),
        })
    }
}

/// Read a whole settings file. Fails (never panics) on anything it doesn't understand.
pub fn read(bytes: &[u8]) -> Result<Blue, String> {
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
    let mut reader = Reader { bytes, at: 5, end, tail, shared: 0, slots: HashMap::new() };
    let value = reader.value(0)?;
    if reader.at != end {
        return Err(format!("{} bytes left over", end - reader.at));
    }
    if reader.shared != count {
        return Err(format!("found {} shared objects, the file says {count}", reader.shared));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// header (1 shared), dict of 2: ("audio": (stamp, 0.5)), ("ui": {"turretsEnabled": (stamp, 1)}),
    /// with "audio" stored shared and used again by REF inside ui.
    fn sample() -> Vec<u8> {
        let stamp = 134_394_328_018_117_906i64.to_le_bytes();
        let mut b = vec![0x7e, 1, 0, 0, 0];
        b.extend([0x16, 2]);
        // entry 1 value: (stamp, 0.5)
        b.extend([0x2c, 0x2f, 8]);
        b.extend(stamp);
        b.push(0x0a);
        b.extend(0.5f64.to_le_bytes());
        // entry 1 key: "audio", shared
        b.extend([0x13 | 0x40, 5]);
        b.extend(b"audio");
        // entry 2 value: dict of 2
        b.extend([0x16, 2]);
        b.extend([0x2c, 0x2f, 8]);
        b.extend(stamp);
        b.push(0x09);
        b.extend([0x13, 14]);
        b.extend(b"turretsEnabled");
        b.push(0x1f);
        b.extend([0x1b, 1]);
        // entry 2 key: "ui"
        b.extend([0x13, 2]);
        b.extend(b"ui");
        // the shared slot table
        b.extend(1u32.to_le_bytes());
        b
    }

    #[test]
    fn reads_sections_stamps_and_shared_keys() {
        let root = read(&sample()).unwrap();
        assert_eq!(unstamped(root.get("audio").unwrap()), &Blue::Float(0.5));
        let ui = root.get("ui").unwrap();
        assert_eq!(unstamped(ui.get("turretsEnabled").unwrap()), &Blue::Int(1));
        assert_eq!(ui.get("audio"), Some(&Blue::Bool(true)));
    }

    #[test]
    fn refuses_broken_files_without_panicking() {
        let good = sample();
        assert!(read(&good[..good.len() - 9]).is_err());
        assert!(read(b"hello").is_err());
        let mut wrong_count = good.clone();
        wrong_count[1] = 2;
        assert!(read(&wrong_count).is_err());
        for cut in 6..good.len() {
            let _ = read(&good[..cut]);
        }
    }

    #[test]
    fn text_is_stable_and_short() {
        let a = Blue::Dict(vec![(Blue::Str("b".into()), Blue::Int(1)), (Blue::Str("a".into()), Blue::Float(0.25))]);
        let b = Blue::Dict(vec![(Blue::Str("a".into()), Blue::Float(0.25)), (Blue::Str("b".into()), Blue::Int(1))]);
        assert_eq!(a.text(), "{a: 0.25, b: 1}");
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_eq!(Blue::Float(0.258).text(), "0.258");
        assert_eq!(Blue::Float(1.0).text(), "1.0");
    }
}
