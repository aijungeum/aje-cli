use std::collections::HashMap;
use std::fs;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct DictEntry {
    pub order: u32,
    pub freq: usize,
    pub token: u64,
    pub lean_pattern: String,
    pub lean_emit: String,
    pub ko_desc: String,
}

pub struct MathlibDict {
    pub entries: Vec<DictEntry>,
    pub by_token: HashMap<u64, DictEntry>,
}

const BIN_MAGIC: &[u8; 8] = b"AJEDICT2";

impl MathlibDict {
    pub fn load_from_file(json_path: &str) -> Result<Self, String> {
        let bin_path = format!("{}.bin", json_path);
        let json_p = Path::new(json_path);
        let bin_p = Path::new(&bin_path);

        if bin_p.exists() {
            let use_bin = match (fs::metadata(json_p), fs::metadata(bin_p)) {
                (Ok(jm), Ok(bm)) => match (jm.modified(), bm.modified()) {
                    (Ok(jt), Ok(bt)) => bt >= jt,
                    _ => true,
                },
                (Err(_), Ok(_)) => true,
                _ => false,
            };

            if use_bin {
                if let Ok(dict) = Self::load_binary(&bin_path) {
                    return Ok(dict);
                }
            }
        }

        let content = fs::read_to_string(json_path)
            .map_err(|e| format!("사전 파일('{}') 읽기 실패: {}", json_path, e))?;
        let dict = Self::parse_json(&content)?;
        let _ = dict.save_binary(&bin_path);
        Ok(dict)
    }

    pub fn save_binary(&self, bin_path: &str) -> Result<(), String> {
        let file = fs::File::create(bin_path).map_err(|e| e.to_string())?;
        let mut w = BufWriter::with_capacity(1024 * 1024, file);

        w.write_all(BIN_MAGIC).map_err(|e| e.to_string())?;
        w.write_all(&(self.entries.len() as u64).to_le_bytes())
            .map_err(|e| e.to_string())?;

        for e in &self.entries {
            w.write_all(&e.order.to_le_bytes())
                .map_err(|e| e.to_string())?;
            w.write_all(&(e.freq as u64).to_le_bytes())
                .map_err(|e| e.to_string())?;
            w.write_all(&e.token.to_le_bytes())
                .map_err(|e| e.to_string())?;
            write_str(&mut w, &e.lean_pattern)?;
            write_str(&mut w, &e.lean_emit)?;
            write_str(&mut w, &e.ko_desc)?;
        }
        w.flush().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn load_binary(bin_path: &str) -> Result<Self, String> {
        let file = fs::File::open(bin_path).map_err(|e| e.to_string())?;
        let mut r = BufReader::with_capacity(1024 * 1024, file);

        let mut magic = [0u8; 8];
        r.read_exact(&mut magic).map_err(|e| e.to_string())?;
        if &magic != BIN_MAGIC {
            return Err("invalid bin version".into());
        }

        let mut count_buf = [0u8; 8];
        r.read_exact(&mut count_buf).map_err(|e| e.to_string())?;
        let count = u64::from_le_bytes(count_buf) as usize;

        let mut entries = Vec::with_capacity(count);
        let mut by_token = HashMap::with_capacity(count);

        for _ in 0..count {
            let mut order_buf = [0u8; 4];
            let mut freq_buf = [0u8; 8];
            let mut token_buf = [0u8; 8];
            r.read_exact(&mut order_buf).map_err(|e| e.to_string())?;
            r.read_exact(&mut freq_buf).map_err(|e| e.to_string())?;
            r.read_exact(&mut token_buf).map_err(|e| e.to_string())?;

            let order = u32::from_le_bytes(order_buf);
            let freq = u64::from_le_bytes(freq_buf) as usize;
            let token = u64::from_le_bytes(token_buf);
            let lean_pattern = read_str(&mut r)?;
            let lean_emit = read_str(&mut r)?;
            let ko_desc = read_str(&mut r)?;

            let entry = DictEntry {
                order,
                freq,
                token,
                lean_pattern,
                lean_emit,
                ko_desc,
            };
            by_token.insert(token, entry.clone());
            entries.push(entry);
        }

        Ok(MathlibDict { entries, by_token })
    }

    pub fn parse_json(raw: &str) -> Result<Self, String> {
        let mut entries = Vec::new();
        let mut by_token = HashMap::new();

        let mut depth = 0;
        let mut obj_start = None;

        for (i, c) in raw.char_indices() {
            if c == '{' {
                if depth == 0 {
                    obj_start = Some(i + 1);
                }
                depth += 1;
            } else if c == '}' {
                depth -= 1;
                if depth == 0 {
                    if let Some(start) = obj_start {
                        let block = &raw[start..i];
                        let entry = Self::parse_object_block(block)?;
                        by_token.insert(entry.token, entry.clone());
                        entries.push(entry);
                    }
                    obj_start = None;
                }
            }
        }

        if entries.is_empty() {
            return Err("사전 파일에서 유효한 매핑 항목을 찾지 못했습니다.".into());
        }

        entries.sort_by_key(|e| e.order);
        Ok(MathlibDict { entries, by_token })
    }

    fn parse_object_block(block: &str) -> Result<DictEntry, String> {
        let order_str = extract_raw_value(block, "order").unwrap_or_else(|| "9999".into());
        let order = order_str.trim().parse::<u32>().unwrap_or(9999);

        let freq_str = extract_raw_value(block, "freq").unwrap_or_else(|| "0".into());
        let freq = freq_str.trim().parse::<usize>().unwrap_or(0);

        let token_str = extract_string_value(block, "token")
            .ok_or_else(|| "항목에 'token' 필드가 누락되었습니다.".to_string())?;
        let clean_hex = token_str.trim_start_matches("0x").trim_start_matches("0X");
        let token = u64::from_str_radix(clean_hex, 16)
            .map_err(|_| format!("잘못된 16진수 토큰 값: {}", token_str))?;

        let lean_pattern = extract_string_value(block, "lean_pattern")
            .ok_or_else(|| format!("토큰 0x{:X}에 'lean_pattern'이 누락되었습니다.", token))?;
        let lean_emit = extract_string_value(block, "lean_emit")
            .unwrap_or_else(|| format!("  apply mathlib_lemma_{:X}\n", token));
        let ko_desc = extract_string_value(block, "ko_desc")
            .unwrap_or_else(|| format!("[매크로 0x{:X}] 수학 정리를 적용합니다.", token));

        Ok(DictEntry {
            order,
            freq,
            token,
            lean_pattern,
            lean_emit,
            ko_desc,
        })
    }
}

fn write_str<W: Write>(w: &mut W, s: &str) -> Result<(), String> {
    let bytes = s.as_bytes();
    w.write_all(&(bytes.len() as u32).to_le_bytes())
        .map_err(|e| e.to_string())?;
    w.write_all(bytes).map_err(|e| e.to_string())?;
    Ok(())
}

fn read_str<R: Read>(r: &mut R) -> Result<String, String> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).map_err(|e| e.to_string())?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).map_err(|e| e.to_string())?;
    String::from_utf8(buf).map_err(|e| e.to_string())
}

fn extract_raw_value(block: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let pos = block.find(&needle)?;
    let rest = &block[pos + needle.len()..];
    let colon = rest.find(':')?;
    let after_colon = rest[colon + 1..].trim_start();
    let end = after_colon.find(',').unwrap_or(after_colon.len());
    Some(after_colon[..end].trim().to_string())
}

fn extract_string_value(block: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let pos = block.find(&needle)?;
    let rest = &block[pos + needle.len()..];
    let colon = rest.find(':')?;
    let after_colon = rest[colon + 1..].trim_start();
    if !after_colon.starts_with('"') {
        return None;
    }

    let mut out = String::new();
    let mut escape = false;
    for c in after_colon.chars().skip(1) {
        if escape {
            match c {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                other => out.push(other),
            }
            escape = false;
        } else if c == '\\' {
            escape = true;
        } else if c == '"' {
            return Some(out);
        } else {
            out.push(c);
        }
    }
    None
}
