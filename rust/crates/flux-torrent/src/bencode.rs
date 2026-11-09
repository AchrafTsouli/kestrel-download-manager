//! A small bencode reader with depth and size limits; reports where the top-level "info" value
//! lies (its bytes are the torrent's identity).

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Dict(BTreeMap<Vec<u8>, Value>),
}

impl Value {
    pub fn get(&self, key: &[u8]) -> Option<&Value> {
        self.as_dict()?.get(key)
    }
    pub fn as_dict(&self) -> Option<&BTreeMap<Vec<u8>, Value>> {
        if let Value::Dict(d) = self { Some(d) } else { None }
    }
    pub fn as_list(&self) -> Option<&Vec<Value>> {
        if let Value::List(l) = self { Some(l) } else { None }
    }
    pub fn as_bytes(&self) -> Option<&[u8]> {
        if let Value::Bytes(b) = self { Some(b) } else { None }
    }
    pub fn as_int(&self) -> Option<i64> {
        if let Value::Int(i) = self { Some(*i) } else { None }
    }
}

/// The whole document and the byte span of its top-level "info" value.
pub fn read_top(data: &[u8]) -> Option<(Value, Option<(usize, usize)>)> {
    let mut span = None;
    let mut i = 0;
    let value = read(data, &mut i, 0, &mut Some(&mut span))?;
    Some((value, span))
}

fn read(data: &[u8], i: &mut usize, depth: usize, info: &mut Option<&mut Option<(usize, usize)>>) -> Option<Value> {
    if depth > 64 || *i >= data.len() {
        return None;
    }
    match data[*i] {
        b'i' => {
            let end = *i + data[*i..].iter().position(|b| *b == b'e')?;
            let value = std::str::from_utf8(&data[*i + 1..end]).ok()?.parse().ok()?;
            *i = end + 1;
            Some(Value::Int(value))
        }
        b'l' => {
            *i += 1;
            let mut list = Vec::new();
            while *i < data.len() && data[*i] != b'e' {
                list.push(read(data, i, depth + 1, &mut None)?);
            }
            (*i < data.len()).then(|| *i += 1)?;
            Some(Value::List(list))
        }
        b'd' => {
            *i += 1;
            let mut dict = BTreeMap::new();
            while *i < data.len() && data[*i] != b'e' {
                let Value::Bytes(key) = read(data, i, depth + 1, &mut None)? else { return None };
                let start = *i;
                let value = read(data, i, depth + 1, &mut None)?;
                if key == b"info" {
                    if let Some(span) = info.as_deref_mut() {
                        *span = Some((start, *i));
                    }
                }
                dict.insert(key, value);
            }
            (*i < data.len()).then(|| *i += 1)?;
            Some(Value::Dict(dict))
        }
        b'0'..=b'9' => {
            let colon = *i + data[*i..].iter().position(|b| *b == b':')?;
            let length: usize = std::str::from_utf8(&data[*i..colon]).ok()?.parse().ok()?;
            let end = (colon + 1).checked_add(length).filter(|e| *e <= data.len())?;
            let bytes = data[colon + 1..end].to_vec();
            *i = end;
            Some(Value::Bytes(bytes))
        }
        _ => None,
    }
}
