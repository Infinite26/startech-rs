use std::io::{self, Read, Write};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    End,
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List(Vec<Value>),
    Compound(Vec<(String, Value)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Value {
    pub fn compound(entries: Vec<(&str, Value)>) -> Value {
        Value::Compound(entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn get<'a>(&'a self, key: &str) -> Option<&'a Value> {
        match self {
            Value::Compound(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Byte(v) => Some(*v as i64),
            Value::Short(v) => Some(*v as i64),
            Value::Int(v) => Some(*v as i64),
            Value::Long(v) => Some(*v),
            Value::Float(v) => Some(*v as i64),
            Value::Double(v) => Some(*v as i64),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        self.as_i64().map(|v| v as i32)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_long_array(&self) -> Option<&Vec<i64>> {
        match self {
            Value::LongArray(a) => Some(a),
            _ => None,
        }
    }

    pub fn tag_id(&self) -> u8 {
        match self {
            Value::End => 0,
            Value::Byte(_) => 1,
            Value::Short(_) => 2,
            Value::Int(_) => 3,
            Value::Long(_) => 4,
            Value::Float(_) => 5,
            Value::Double(_) => 6,
            Value::ByteArray(_) => 7,
            Value::String(_) => 8,
            Value::List(_) => 9,
            Value::Compound(_) => 10,
            Value::IntArray(_) => 11,
            Value::LongArray(_) => 12,
        }
    }
}

fn read_u8<R: Read>(r: &mut R) -> io::Result<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}

fn read_i16<R: Read>(r: &mut R) -> io::Result<i16> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(i16::from_be_bytes(b))
}

fn read_i32<R: Read>(r: &mut R) -> io::Result<i32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(i32::from_be_bytes(b))
}

fn read_i64<R: Read>(r: &mut R) -> io::Result<i64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(i64::from_be_bytes(b))
}

fn read_f32<R: Read>(r: &mut R) -> io::Result<f32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(f32::from_be_bytes(b))
}

fn read_f64<R: Read>(r: &mut R) -> io::Result<f64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(f64::from_be_bytes(b))
}

fn read_string<R: Read>(r: &mut R) -> io::Result<String> {
    let len = read_u16(r)? as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn read_u16<R: Read>(r: &mut R) -> io::Result<u16> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_be_bytes(b))
}

fn read_payload<R: Read>(r: &mut R, tag: u8) -> io::Result<Value> {
    Ok(match tag {
        0 => Value::End,
        1 => Value::Byte(read_u8(r)? as i8),
        2 => Value::Short(read_i16(r)?),
        3 => Value::Int(read_i32(r)?),
        4 => Value::Long(read_i64(r)?),
        5 => Value::Float(read_f32(r)?),
        6 => Value::Double(read_f64(r)?),
        7 => {
            let n = read_i32(r)?;
            if n < 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "negative byte array length"));
            }
            let mut buf = vec![0u8; n as usize];
            r.read_exact(&mut buf)?;
            Value::ByteArray(buf.into_iter().map(|b| b as i8).collect())
        }
        8 => Value::String(read_string(r)?),
        9 => {
            let elem = read_u8(r)?;
            let n = read_i32(r)?;
            if n < 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "negative list length"));
            }
            let mut items = Vec::with_capacity(n.min(4096) as usize);
            for _ in 0..n {
                items.push(read_payload(r, elem)?);
            }
            Value::List(items)
        }
        10 => {
            let mut entries = Vec::new();
            loop {
                let t = read_u8(r)?;
                if t == 0 {
                    break;
                }
                let name = read_string(r)?;
                entries.push((name, read_payload(r, t)?));
            }
            Value::Compound(entries)
        }
        11 => {
            let n = read_i32(r)?;
            if n < 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "negative int array length"));
            }
            let mut v = Vec::with_capacity(n.min(1 << 20) as usize);
            for _ in 0..n {
                v.push(read_i32(r)?);
            }
            Value::IntArray(v)
        }
        12 => {
            let n = read_i32(r)?;
            if n < 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "negative long array length"));
            }
            let mut v = Vec::with_capacity(n.min(1 << 20) as usize);
            for _ in 0..n {
                v.push(read_i64(r)?);
            }
            Value::LongArray(v)
        }
        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid nbt tag")),
    })
}

/// Parse a full NBT document (named root tag). Returns (root name, value).
pub fn read_named<R: Read>(r: &mut R) -> io::Result<(String, Value)> {
    let tag = read_u8(r)?;
    if tag == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "empty nbt document"));
    }
    let name = read_string(r)?;
    let value = read_payload(r, tag)?;
    Ok((name, value))
}

/// Parse a full NBT document, discarding the root name.
pub fn read<R: Read>(r: &mut R) -> io::Result<Value> {
    Ok(read_named(r)?.1)
}

fn write_u16<W: Write>(w: &mut W, v: u16) -> io::Result<()> {
    w.write_all(&v.to_be_bytes())
}

fn write_string<W: Write>(w: &mut W, s: &str) -> io::Result<()> {
    let bytes = s.as_bytes();
    write_u16(w, bytes.len() as u16)?;
    w.write_all(bytes)
}

fn write_payload<W: Write>(w: &mut W, value: &Value) -> io::Result<()> {
    match value {
        Value::End => Ok(()),
        Value::Byte(v) => w.write_all(&(*v as u8).to_be_bytes()),
        Value::Short(v) => w.write_all(&v.to_be_bytes()),
        Value::Int(v) => w.write_all(&v.to_be_bytes()),
        Value::Long(v) => w.write_all(&v.to_be_bytes()),
        Value::Float(v) => w.write_all(&v.to_be_bytes()),
        Value::Double(v) => w.write_all(&v.to_be_bytes()),
        Value::ByteArray(v) => {
            w.write_all(&(v.len() as i32).to_be_bytes())?;
            for b in v {
                w.write_all(&(*b as u8).to_be_bytes())?;
            }
            Ok(())
        }
        Value::String(s) => write_string(w, s),
        Value::List(items) => {
            let elem = items.first().map(|v| v.tag_id()).unwrap_or(0);
            w.write_all(&elem.to_be_bytes())?;
            w.write_all(&(items.len() as i32).to_be_bytes())?;
            for item in items {
                write_payload(w, item)?;
            }
            Ok(())
        }
        Value::Compound(entries) => {
            for (k, v) in entries {
                w.write_all(&v.tag_id().to_be_bytes())?;
                write_string(w, k)?;
                write_payload(w, v)?;
            }
            w.write_all(&0u8.to_be_bytes())
        }
        Value::IntArray(v) => {
            w.write_all(&(v.len() as i32).to_be_bytes())?;
            for x in v {
                w.write_all(&x.to_be_bytes())?;
            }
            Ok(())
        }
        Value::LongArray(v) => {
            w.write_all(&(v.len() as i32).to_be_bytes())?;
            for x in v {
                w.write_all(&x.to_be_bytes())?;
            }
            Ok(())
        }
    }
}

/// Serialize an NBT document with the given root name.
pub fn write_named<W: Write>(w: &mut W, name: &str, value: &Value) -> io::Result<()> {
    w.write_all(&value.tag_id().to_be_bytes())?;
    write_string(w, name)?;
    write_payload(w, value)
}

/// Serialize an NBT document with an empty root name (network NBT style used by 1.20.1 packets).
pub fn write<W: Write>(w: &mut W, value: &Value) -> io::Result<()> {
    write_named(w, "", value)
}

/// Serialize to a byte vector with an empty root name.
pub fn to_bytes(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    let _ = write(&mut out, value);
    out
}
