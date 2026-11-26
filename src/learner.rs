use crate::dict::{DictEntry, MathlibDict};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

fn collect_lean_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries =
        fs::read_dir(dir).map_err(|e| format!("디렉토리('{}') 읽기 실패: {}", dir.display(), e))?;

    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }
            subdirs.push(path);
        } else if path.extension().and_then(|e| e.to_str()) == Some("lean") {
            out.push(path);
        }
    }

    for sub in subdirs {
        collect_lean_files(&sub, out)?;
    }
    Ok(())
}

fn strip_lean_comments(src: &str) -> String {
    let mut result = String::with_capacity(src.len());
    let mut block_depth = 0usize;

    for line in src.lines() {
        let mut i = 0;
        let bytes = line.as_bytes();
        while i < bytes.len() {
            if i + 1 < bytes.len() && bytes.get(i) == Some(&b'/') && bytes.get(i + 1) == Some(&b'-')
            {
                block_depth += 1;
                i += 2;
            } else if i + 1 < bytes.len()
                && bytes.get(i) == Some(&b'-')
                && bytes.get(i + 1) == Some(&b'/')
                && block_depth > 0
            {
                block_depth -= 1;
                i += 2;
            } else if block_depth == 0
                && i + 1 < bytes.len()
                && bytes.get(i) == Some(&b'-')
                && bytes.get(i + 1) == Some(&b'-')
            {
                break;
            } else {
                if block_depth == 0 {
                    if let Some(ch) = line.get(i..).and_then(|s| s.chars().next()) {
                        result.push(ch);
                        i += ch.len_utf8();
                        continue;
                    }
                }
                i += 1;
            }
        }
        result.push('\n');
    }
    result
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '\''
}

fn is_reserved_keyword(word: &str) -> bool {
    matches!(
        word,
        "theorem"
            | "lemma"
            | "def"
            | "import"
            | "open"
            | "section"
            | "namespace"
            | "variable"
            | "variables"
            | "where"
            | "match"
            | "with"
            | "fun"
            | "let"
            | "have"
            | "show"
            | "from"
            | "by"
            | "exact"
            | "apply"
            | "rw"
            | "simp"
            | "ring"
            | "linarith"
            | "omega"
            | "norm_num"
            | "intro"
            | "intros"
            | "cases"
            | "rcases"
            | "obtain"
            | "refine"
            | "constructor"
            | "ext"
            | "congr"
            | "calc"
            | "by_cases"
            | "by_contra"
            | "Prop"
            | "Type"
            | "Sort"
            | "true"
            | "false"
            | "sorry"
    )
}

pub fn learn_and_update_dict(
    src_dir: &str,
    dict_path: &str,
    max_limit: Option<usize>,
    json_limit: Option<usize>,
) -> Result<(usize, usize, usize), String> {
    let root = Path::new(src_dir);
    if !root.exists() {
        return Err(format!("소스 경로('{}')가 존재하지 않습니다.", src_dir));
    }

    let mut files = Vec::new();
    collect_lean_files(root, &mut files)?;
    if files.is_empty() {
        return Err(format!(
            "'{}' 경로에서 .lean 파일을 찾을 수 없습니다.",
            src_dir
        ));
    }

    files.sort_by(|a, b| {
        let a_is_main = a.file_name().and_then(|n| n.to_str()) == Some("Main.lean");
        let b_is_main = b.file_name().and_then(|n| n.to_str()) == Some("Main.lean");
        b_is_main.cmp(&a_is_main).then_with(|| a.cmp(b))
    });

    let mut dict = if Path::new(dict_path).exists() {
        MathlibDict::load_from_file(dict_path)?
    } else {
        MathlibDict {
            entries: Vec::new(),
            by_token: HashMap::new(),
        }
    };

    let existing_patterns: HashSet<String> = dict
        .entries
        .iter()
        .map(|e| e.lean_pattern.clone())
        .collect();

    // 식별자 빈도, theorem/lemma 정리 선언 여부(1회 출현이더라도 최우선 보호) 동시 집계
    let mut freq_map: HashMap<String, usize> = HashMap::new();
    let mut declared_theorems: HashSet<String> = HashSet::new();

    for file in &files {
        let raw = fs::read_to_string(file)
            .map_err(|e| format!("파일('{}') 읽기 오류: {}", file.display(), e))?;
        let clean = strip_lean_comments(&raw);

        let mut prev_was_decl = false;
        for raw_tok in clean.split(|c: char| !is_ident_char(c)) {
            let tok = raw_tok.trim_matches('.');
            if tok.is_empty() {
                continue;
            }
            if tok == "theorem" || tok == "lemma" {
                prev_was_decl = true;
                continue;
            }
            if tok.len() < 4 || is_reserved_keyword(tok) {
                prev_was_decl = false;
                continue;
            }
            if tok
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(true)
            {
                prev_was_decl = false;
                continue;
            }

            if prev_was_decl {
                declared_theorems.insert(tok.to_string());
                prev_was_decl = false;
            }

            if !existing_patterns.contains(tok) {
                *freq_map.entry(tok.to_string()).or_insert(0) += 1;
            }
        }
    }

    let total_unique_found = freq_map.len();

    // 1순위: 직접 선언된 theorem/lemma 여부
    // 2순위: 출현 빈도
    // 3순위: 이름순
    let mut ranked: Vec<(String, usize, bool)> = freq_map
        .into_iter()
        .map(|(k, v)| {
            let is_decl = declared_theorems.contains(&k);
            (k, v, is_decl)
        })
        .collect();

    ranked.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.0.cmp(&b.0))
    });

    let limit = match max_limit {
        Some(n) if n > 0 => n,
        _ => usize::MAX,
    };

    let mut candidate_token: u64 = 0x0001;
    let mut next_order: u32 = dict.entries.iter().map(|e| e.order).max().unwrap_or(0) + 1;
    let mut added_count = 0usize;

    // 3원소 튜플 (ident, count, is_decl) 언패킹  //ranked (String, usize, bool)
    for (ident, count, is_decl) in ranked.into_iter().take(limit) {
        // 기존 사전을 불러와 추가로 학습(learn-dict)할 때,
        //   이미 사전에 등록된 토큰 ID와 겹치지 않도록, 비어 있는 가장 빠른 다음 토큰 번호를 찾아 배정
        while dict.by_token.contains_key(&candidate_token) {
            candidate_token += 1;
        }

        let glyph_len = if candidate_token <= 0x7FFF {
            1
        } else if candidate_token <= 0x3FFF_FFFF {
            2
        } else {
            3
        };

        let tag = if is_decl {
            "핵심 선언 정리"
        } else {
            "자동 학습 식별자"
        };

        let entry = DictEntry {
            order: next_order,
            freq: count,
            token: candidate_token,
            lean_pattern: ident.clone(),
            lean_emit: format!("  apply {}\n", ident),
            ko_desc: format!(
                "[{}({}글자 조합)] `{}` (출현 {}회)를 적용합니다.",
                tag, glyph_len, ident, count
            ),
        };

        dict.by_token.insert(candidate_token, entry.clone());
        dict.entries.push(entry);

        candidate_token += 1;
        next_order += 1;
        added_count += 1;
    }

    // 전체 항목은 cache 바이너리 사전(.bin)에 100% 저장
    dict.entries.sort_by_key(|e| e.order);
    let bin_path = format!("{}.bin", dict_path);
    dict.save_binary(&bin_path)?;

    // json_limit: 미지정시 전체, 지정시 해당 갯수의 사전 생성
    save_dict_json(&dict.entries, dict_path, json_limit)?;

    Ok((files.len(), total_unique_found, added_count))
}

fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

/// 사전 항목을 JSON 파일로 저장 (limit이 Some(n)이면 상위 n개만, None이면 전체 저장)
pub fn save_dict_json(
    entries: &[DictEntry],
    dict_path: &str,
    limit: Option<usize>,
) -> Result<(), String> {
    let json_out = match limit {
        Some(n) if n > 0 && n < entries.len() => serialize_dict_to_json(&entries[..n]),
        _ => serialize_dict_to_json(entries),
    };
    fs::write(dict_path, json_out)
        .map_err(|e| format!("사전 JSON 파일('{}') 저장 실패: {}", dict_path, e))
}

pub fn serialize_dict_to_json(entries: &[DictEntry]) -> String {
    let mut out = String::from("[\n");
    for (idx, e) in entries.iter().enumerate() {
        writeln!(out, "  {{").unwrap();
        writeln!(out, "    \"order\": {},", e.order).unwrap();
        writeln!(out, "    \"freq\": {},", e.freq).unwrap();
        writeln!(out, "    \"token\": \"0x{:X}\",", e.token).unwrap();
        writeln!(
            out,
            "    \"lean_pattern\": \"{}\",",
            escape_json(&e.lean_pattern)
        )
        .unwrap();
        writeln!(out, "    \"lean_emit\": \"{}\",", escape_json(&e.lean_emit)).unwrap();
        writeln!(out, "    \"ko_desc\": \"{}\"", escape_json(&e.ko_desc)).unwrap();
        if idx + 1 < entries.len() {
            writeln!(out, "  }},").unwrap();
        } else {
            writeln!(out, "  }}").unwrap();
        }
    }
    out.push_str("]\n");
    out
}

/// 사전의 상위 N개 항목을 JSON 파일로 저장 (edit-dict 및 learn-dict 공용)
pub fn save_top_json(entries: &[DictEntry], dict_path: &str, limit: usize) -> Result<(), String> {
    let top_entries: Vec<DictEntry> = entries.iter().take(limit).cloned().collect();
    let json_out = serialize_dict_to_json(&top_entries);
    fs::write(dict_path, json_out)
        .map_err(|e| format!("사전 JSON 파일('{}') 저장 실패: {}", dict_path, e))
}
