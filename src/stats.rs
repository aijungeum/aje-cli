use crate::compiler::MODULE_HEADER_TOKEN;
use crate::decompiler::parse_module_header;
use crate::dict::MathlibDict;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

fn collect_lean_bytes(path: &Path) -> Result<(usize, usize), String> {
    if path.is_file() {
        let meta = fs::metadata(path).map_err(|e| e.to_string())?;
        return Ok((1, meta.len() as usize));
    }
    if path.is_dir() {
        let mut total_files = 0usize;
        let mut total_bytes = 0usize;
        let entries = fs::read_dir(path).map_err(|e| e.to_string())?;
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with('.') {
                        continue;
                    }
                }
                let (f, b) = collect_lean_bytes(&p)?;
                total_files += f;
                total_bytes += b;
            } else if p.extension().and_then(|e| e.to_str()) == Some("lean") {
                if let Ok(m) = fs::metadata(&p) {
                    total_files += 1;
                    total_bytes += m.len() as usize;
                }
            }
        }
        return Ok((total_files, total_bytes));
    }
    Err(format!(
        "원본 경로('{}')를 찾을 수 없습니다.",
        path.display()
    ))
}

pub fn print_aje_stats(
    aje_content: &str,
    tokens: &[u64],
    dict: &MathlibDict,
    orig_path: Option<&str>,
    top_n: usize,
) -> Result<(), String> {
    let total_glyphs = aje_content.chars().count();
    let utf8_bytes = aje_content.len();

    let mut module_count = 0usize;
    let mut active_tokens = 0usize;
    let mut len1_count = 0usize;
    let mut len2_count = 0usize;
    let mut len3_count = 0usize;
    let mut freq_map: HashMap<u64, usize> = HashMap::new();

    let mut i = 0usize;
    while i < tokens.len() {
        let t = tokens[i];
        if t == MODULE_HEADER_TOKEN {
            if let Ok((_, consumed)) = parse_module_header(tokens, i) {
                module_count += 1;
                i += consumed;
                continue;
            }
        }

        active_tokens += 1;
        *freq_map.entry(t).or_insert(0) += 1;

        if t <= 0x7FFF {
            len1_count += 1;
        } else if t <= 0x3FFF_FFFF {
            len2_count += 1;
        } else {
            len3_count += 1;
        }
        i += 1;
    }

    if module_count == 0 {
        module_count = 1;
    }

    println!("==================================================================");
    println!("               AI정음(AiJungEum) 압축 및 논리 통계 리포트");
    println!("==================================================================");
    println!(
        "▶ 복원 가능 모듈(파일) 수: {} 개 (원본 상대경로 포함)",
        module_count
    );
    println!(
        "▶ 순수 Lean 수학 논리 토큰 수: {} 개 (경로 헤더 포함 전체: {} 개)",
        active_tokens,
        tokens.len()
    );
    println!(
        "▶ AI정음 총 글자 수     : {} 글자 (UTF-8 인코딩: {} bytes)",
        total_glyphs, utf8_bytes
    );
    println!(
        "▶ 가변 길이 조합 분포   : 1글자(15bit): {}개 | 2글자(30bit): {}개 | 3글자(45bit): {}개",
        len1_count, len2_count, len3_count
    );

    if let Some(orig) = orig_path {
        let (orig_files, orig_bytes) = collect_lean_bytes(Path::new(orig))?;
        if orig_bytes > 0 {
            let char_reduction = (1.0 - (total_glyphs as f64 / orig_bytes as f64)) * 100.0;
            let byte_reduction = (1.0 - (utf8_bytes as f64 / orig_bytes as f64)) * 100.0;
            println!("------------------------------------------------------------------");
            println!(
                "▶ 원본 Lean 규모      : {} 개 파일 / {} bytes (약 {} 자)",
                orig_files, orig_bytes, orig_bytes
            );
            println!(
                "▶ 표현 길이(글자수) 압축: {} 자 -> {} 글자 ({:.2}% 감소, {:.1}배 압축)",
                orig_bytes,
                total_glyphs,
                char_reduction,
                orig_bytes as f64 / total_glyphs.max(1) as f64
            );
            println!(
                "▶ 실제 저장 용량(UTF-8) : {} bytes -> {} bytes ({:.2}% 감소)",
                orig_bytes, utf8_bytes, byte_reduction
            );
        }
    }

    println!("==================================================================");
    println!("               상위 출현 핵심 Mathlib 매크로 Top {}", top_n);
    println!("==================================================================");

    let mut ranked: Vec<(u64, usize)> = freq_map.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    for (rank, (token_id, count)) in ranked.into_iter().take(top_n).enumerate() {
        let pct = (count as f64 / active_tokens.max(1) as f64) * 100.0;
        if let Some(entry) = dict.by_token.get(&token_id) {
            println!(
                "{:>2}. [0x{:05X}] {:<32} | {:>4}회 ({:>5.1}%) | {}",
                rank + 1,
                token_id,
                entry.lean_pattern,
                count,
                pct,
                entry.ko_desc
            );
        } else {
            println!(
                "{:>2}. [0x{:05X}] (미등록 식별자)                  | {:>4}회 ({:>5.1}%)",
                rank + 1,
                token_id,
                count,
                pct
            );
        }
    }
    println!("==================================================================");

    Ok(())
}
