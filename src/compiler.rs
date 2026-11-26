use crate::dict::{DictEntry, MathlibDict};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// 새로운 파일(모듈) 블록의 시작과 경로 메타데이터 헤더를 알리는 토큰 (0x0000은 사전에서 미사용)
pub const MODULE_HEADER_TOKEN: u64 = 0x0000;

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '\''
}

fn strip_comments_fast(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let bytes = src.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut block_depth = 0usize;

    while i < len {
        if i + 1 < len && bytes.get(i) == Some(&b'/') && bytes.get(i + 1) == Some(&b'-') {
            block_depth += 1;
            i += 2;
        } else if i + 1 < len
            && bytes.get(i) == Some(&b'-')
            && bytes.get(i + 1) == Some(&b'/')
            && block_depth > 0
        {
            block_depth -= 1;
            i += 2;
        } else if block_depth == 0
            && i + 1 < len
            && bytes.get(i) == Some(&b'-')
            && bytes.get(i + 1) == Some(&b'-')
        {
            while i < len && bytes.get(i) != Some(&b'\n') {
                i += 1;
            }
        } else {
            if block_depth == 0 {
                if let Some(ch) = src.get(i..).and_then(|s| s.chars().next()) {
                    out.push(ch);
                    i += ch.len_utf8();
                    continue;
                }
            }
            i += 1;
        }
    }
    out
}

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

/// 직렬화: [0x0000, byte_len, byte_0, byte_1, ...] 1바이트 시퀀스로 직렬화
///    맨앞 0x0000 : 상위 레이어. 일반 사전 토큰과 파일 상대경로를 구분
///    본문 1바이트: 하위 레이어. 1비트(가변여부), 15비트(패킹)
pub fn encode_module_header(rel_path: &str) -> Vec<u64> {
    let bytes = rel_path.as_bytes();
    let mut header = Vec::with_capacity(2 + bytes.len());
    header.push(MODULE_HEADER_TOKEN);
    header.push(bytes.len() as u64);

    for &b in bytes {
        header.push(b as u64); // 1바이트 범위이므로 15비트 가변 인코더에서 절대 손실되지 않음
    }
    header
}

pub struct FastMatcher<'a> {
    ident_map: HashMap<&'a str, (u32, u64)>,
    phrase_patterns: Vec<&'a DictEntry>,
}

impl<'a> FastMatcher<'a> {
    pub fn new(dict: &'a MathlibDict) -> Self {
        let mut ident_map = HashMap::with_capacity(dict.entries.len());
        let mut phrase_patterns = Vec::new();

        for entry in &dict.entries {
            let pat = entry.lean_pattern.trim();
            if pat.is_empty() {
                continue;
            }
            if pat.chars().all(is_ident_char) {
                ident_map.insert(pat, (entry.order, entry.token));
            } else {
                phrase_patterns.push(entry);
            }
        }

        FastMatcher {
            ident_map,
            phrase_patterns,
        }
    }

    pub fn match_source(&self, source: &str) -> Vec<u64> {
        let clean_src = strip_comments_fast(source);
        let mut code_lines = Vec::new();
        for line in clean_src.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with("import ") {
                code_lines.push(trimmed);
            }
        }
        let normalized = code_lines.join(" ");
        if normalized.is_empty() {
            return Vec::new();
        }

        let mut matched: Vec<(u32, u64)> = Vec::new();
        let mut seen_tokens: HashSet<u64> = HashSet::new();

        for raw_tok in normalized.split(|c: char| !is_ident_char(c)) {
            let tok = raw_tok.trim_matches('.');
            if tok.is_empty() {
                continue;
            }
            if let Some(&(order, token_id)) = self.ident_map.get(tok) {
                if seen_tokens.insert(token_id) {
                    matched.push((order, token_id));
                }
            }
        }

        for entry in &self.phrase_patterns {
            if normalized.contains(&entry.lean_pattern) && seen_tokens.insert(entry.token) {
                matched.push((entry.order, entry.token));
            }
        }

        matched.sort_by_key(|&(order, _)| order);
        matched.into_iter().map(|(_, token_id)| token_id).collect()
    }
}

pub fn compile_lean4(source: &str, dict: &MathlibDict) -> Result<Vec<u64>, String> {
    let start_time = Instant::now();
    let matcher = FastMatcher::new(dict);
    let tokens = matcher.match_source(source);

    if tokens.is_empty() {
        return Err("사전에 매칭되는 Lean 4 구문을 찾지 못했습니다.".into());
    }

    let elapsed = start_time.elapsed();
    println!(
        "  -> [고속 파서 엔진] 사전 {}개 항목 중 {}개 매칭 완료 (소요 시간: {:.3}ms)",
        dict.entries.len(),
        tokens.len(),
        elapsed.as_secs_f64() * 1000.0
    );

    Ok(tokens)
}

/// Lean 프로젝트 전체를, 경로 헤더 포함, 단일 토큰 스트림으로, 컴파일
pub fn compile_project(
    src_dir: &str,
    dict: &MathlibDict,
) -> Result<(Vec<u64>, usize, usize), String> {
    let start_time = Instant::now();
    let root = Path::new(src_dir);
    if !root.exists() {
        return Err(format!("프로젝트 경로('{}')가 존재하지 않습니다.", src_dir));
    }

    let mut files = Vec::new();
    collect_lean_files(root, &mut files)?;
    if files.is_empty() {
        return Err(format!(
            "'{}' 경로에서 .lean 파일을 찾지 못했습니다.",
            src_dir
        ));
    }

    files.sort_by(|a, b| {
        let a_is_main = a.file_name().and_then(|n| n.to_str()) == Some("Main.lean");
        let b_is_main = b.file_name().and_then(|n| n.to_str()) == Some("Main.lean");
        b_is_main.cmp(&a_is_main).then_with(|| a.cmp(b))
    });

    let matcher = FastMatcher::new(dict);

    let mut combined_tokens = Vec::new();
    let mut processed_files = 0usize;
    let mut total_bytes = 0usize;

    for file in &files {
        let raw = fs::read_to_string(file)
            .map_err(|e| format!("파일('{}') 읽기 실패: {}", file.display(), e))?;
        total_bytes += raw.len();

        let file_tokens = matcher.match_source(&raw);
        if !file_tokens.is_empty() {
            // OS 독립적인 슬래시 / 기반 상대경로 추출
            let rel = file
                .strip_prefix(root)
                .unwrap_or(file)
                .to_string_lossy()
                .replace('\\', "/");

            combined_tokens.extend(encode_module_header(&rel));
            combined_tokens.extend(file_tokens);
            processed_files += 1;
        }
    }

    if combined_tokens.is_empty() {
        return Err("프로젝트 전체에서 매칭되는 Lean 4 구문을 찾지 못했습니다.".into());
    }

    let elapsed = start_time.elapsed();
    println!(
        "  -> [프로젝트 통합 엔진] {}개 파일(경로 메타데이터 내장) / {} bytes -> {}개 통합 토큰 압축 완료 (소요 시간: {:.3}ms)",
        processed_files,
        total_bytes,
        combined_tokens.len(),
        elapsed.as_secs_f64() * 1000.0
    );

    Ok((combined_tokens, processed_files, total_bytes))
}
