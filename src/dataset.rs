use crate::compiler::FastMatcher;
use crate::decompiler::decompile_tokens;
use crate::dict::MathlibDict;
use crate::encode_tokens;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Instant;

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

fn format_jsonl_record(
    task: &str,
    module: &str,
    instruction: &str,
    input: &str,
    output: &str,
) -> String {
    format!(
        "{{\"task\":\"{}\",\"module\":\"{}\",\"instruction\":\"{}\",\"input\":\"{}\",\"output\":\"{}\"}}",
        escape_json(task),
        escape_json(module),
        escape_json(instruction),
        escape_json(input),
        escape_json(output)
    )
}

/// 데이터셋
/// 단일 파일을 처리해 3종의 JSONL 레코드 문자열로 변환
/// Lean <-> AI정음,  AI정음 <-> decompiled, Lean <-> decompiled
fn process_single_file(
    file: &Path,
    root: &Path,
    matcher: &FastMatcher,
    dict: &MathlibDict,
) -> Option<String> {
    let raw_lean = fs::read_to_string(file).ok()?;
    let tokens = matcher.match_source(&raw_lean);
    if tokens.is_empty() {
        return None;
    }

    let aje_stream = encode_tokens(&tokens);
    let decompiled = decompile_tokens(&tokens, dict);
    let rel_path = file
        .strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/");

    let mut block = String::with_capacity(raw_lean.len() * 2);

    block.push_str(&format_jsonl_record(
        "lean_to_aje",
        &rel_path,
        "다음 Lean 4 형식 증명 코드를 고밀도 AI정음(AJE-MathDSL) 토큰 시퀀스로 압축하시오.",
        raw_lean.trim(),
        &aje_stream,
    ));
    block.push('\n');

    block.push_str(&format_jsonl_record(
        "aje_to_lean",
        &rel_path,
        "주어진 AI정음(AJE-MathDSL) 토큰 시퀀스를 해석하여 실행 가능한 Lean 4 형식 증명 코드로 복원하시오.",
        &aje_stream,
        decompiled.lean4_code.trim(),
    ));
    block.push('\n');

    block.push_str(&format_jsonl_record(
        "aje_to_korean",
        &rel_path,
        "주어진 AI정음(AJE-MathDSL) 증명 토큰 시퀀스를 분석하여 비전공자도 이해할 수 있는 단계별 한국어 논리 해설로 설명하시오.",
        &aje_stream,
        decompiled.korean_explanation.trim(),
    ));
    block.push('\n');

    Some(block)
}

/// 파인튜닝 데이터셋 추출기
/// max_modules : 추출할 최대 모듈 수 (샘플링 테스트용, None이면 전체 처리)
pub fn export_training_dataset(
    src_dir: &str,
    dict: &MathlibDict,
    out_jsonl_path: &str,
    max_modules: Option<usize>,
) -> Result<(usize, usize), String> {
    let start_time = Instant::now();
    let root = Path::new(src_dir);
    if !root.exists() {
        return Err(format!("소스 경로('{}')가 존재하지 않습니다.", src_dir));
    }

    let mut files = Vec::new();
    collect_lean_files(root, &mut files)?;
    if files.is_empty() {
        return Err(format!(
            "'{}' 경로에서 .lean 파일을 찾지 못했습니다.",
            src_dir
        ));
    }

    // Main.lean 우선 정렬
    files.sort_by(|a, b| {
        let a_is_main = a.file_name().and_then(|n| n.to_str()) == Some("Main.lean");
        let b_is_main = b.file_name().and_then(|n| n.to_str()) == Some("Main.lean");
        b_is_main.cmp(&a_is_main).then_with(|| a.cmp(b))
    });

    let num_threads = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    println!(
        "  -> [병렬 데이터셋 엔진] CPU {}개 스레드 가동 / 사전 {}개 항목 1회 인덱싱 중...",
        num_threads,
        dict.entries.len()
    );

    // 사전 인덱스 1번만 생성
    let matcher = FastMatcher::new(dict);

    let out_file = fs::File::create(out_jsonl_path)
        .map_err(|e| format!("출력 파일('{}') 생성 실패: {}", out_jsonl_path, e))?;
    let mut writer = BufWriter::with_capacity(1024 * 1024, out_file);

    let target_limit = match max_modules {
        Some(n) if n > 0 => n,
        _ => usize::MAX,
    };

    let mut processed_modules = 0usize;
    let mut total_samples = 0usize;

    // 스레드 수의 4배 단위로 병렬 처리 : max_modules 도달 시 작업 종료
    let batch_size = num_threads * 4;
    for batch in files.chunks(batch_size) {
        if processed_modules >= target_limit {
            break;
        }

        let batch_results: Vec<Option<String>> = thread::scope(|s| {
            let mut handles = Vec::with_capacity(batch.len());
            for file in batch {
                let m_ref = &matcher;
                handles.push(s.spawn(move || process_single_file(file, root, m_ref, dict)));
            }
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or(None))
                .collect()
        });

        for record_block in batch_results.into_iter().flatten() {
            if processed_modules >= target_limit {
                break;
            }
            writer
                .write_all(record_block.as_bytes())
                .map_err(|e| format!("데이터셋 쓰기 오류: {}", e))?;
            processed_modules += 1;
            total_samples += 3;
        }
    }

    writer
        .flush()
        .map_err(|e| format!("파일 버퍼 플러시 오류: {}", e))?;

    if total_samples == 0 {
        return Err("추출할 수 있는 유효한 학습 샘플이 없습니다.".into());
    }

    let elapsed = start_time.elapsed();
    println!(
        "  -> [병렬 처리 완료] 총 소요 시간: {:.3}초",
        elapsed.as_secs_f64()
    );

    Ok((processed_modules, total_samples))
}
