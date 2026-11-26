// Cmd:
// learn-dict, search-dict, edit-dict, split-dict, merge-dict
// compile
// decompile
// compile-project ./lean_src riemann_full.aje
// decompile-project riemann_full.aje ./restored_lean_src
// encode
// decode
// stats riemann_full.aje ./lean_src 20   # 상위 20개 통계
// verify
// export-dataset ./lean_src train.jsonl
// export-dataset ./lean_src train_30.jsonl 30   # 파인튜닝 테스트: 30개만 샘플링 추출

mod collab;
mod compiler;
mod dataset;
mod decompiler;
mod dict;
mod learner;
mod stats;
mod verifier;

use dict::MathlibDict;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::process;

const PUA_A_START: u32 = 0xF0000;
const PUA_A_LIMIT: u32 = 0xF7FFF; // 32,768개 (0xF0000 ~ 0xF7FFF)
const PUA_B_START: u32 = 0x100000;
const PUA_B_LIMIT: u32 = 0x107FFF; // 32,768개 (0x100000 ~ 0x107FFF)
const JUMP_THRESHOLD: u32 = 0x8000;

const DEFAULT_DICT_PATH: &str = "mathlib_dict.json";

#[derive(Debug)]
pub enum AjeError {
    Io(io::Error),
    OddByteLength(usize),
    InvalidCodePoint(u32, usize),
    IncompleteSequence,
    InvalidArgs(String),
    ProcessError(String),
}

impl From<io::Error> for AjeError {
    fn from(err: io::Error) -> Self {
        AjeError::Io(err)
    }
}

impl std::fmt::Display for AjeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AjeError::Io(e) => write!(f, "파일 입출력 오류: {}", e),
            AjeError::OddByteLength(len) => {
                write!(f, "입력 바이트 길이가 홀수입니다 ({} bytes).", len)
            }
            AjeError::InvalidCodePoint(cp, idx) => {
                write!(
                    f,
                    "유효하지 않은 AI정음 코드포인트 (인덱스: {}, U+{:X})",
                    idx, cp
                )
            }
            AjeError::IncompleteSequence => {
                write!(
                    f,
                    "Unexpected EOF: 가변 길이 PUA 시퀀스가 완성되지 않은 채 파일이 끝났습니다."
                )
            }
            AjeError::InvalidArgs(msg) => write!(f, "인자 오류: {}", msg),
            AjeError::ProcessError(msg) => write!(f, "{}", msg),
        }
    }
}

/// 단일 16비트(u16) 글리프를 AI정음 PUA-A/B 유니코드 문자로 변환
#[inline]
pub fn encode_aje_token(val: u16) -> char {
    let v = val as u32;
    let code = if v >= JUMP_THRESHOLD {
        PUA_B_START + (v - JUMP_THRESHOLD)
    } else {
        PUA_A_START + v
    };
    char::from_u32(code).expect("유효하지 않은 유니코드 매핑")
}

/// 단일 AI정음 유니코드 문자를 16비트(u16) 글리프 값으로 디코딩
#[inline]
pub fn decode_aje_char(c: char, index: usize) -> Result<u16, AjeError> {
    let cp = c as u32;
    if (PUA_A_START..=PUA_A_LIMIT).contains(&cp) {
        Ok((cp - PUA_A_START) as u16)
    } else if (PUA_B_START..=PUA_B_LIMIT).contains(&cp) {
        Ok(((cp - PUA_B_START) + JUMP_THRESHOLD) as u16)
    } else {
        Err(AjeError::InvalidCodePoint(cp, index))
    }
}

/// [가변 길이 인코더] u64 ID를 1글자(15bit), 2글자(30bit), 3글자(45bit) AI정음 조합으로 변환
pub fn encode_var_id(mut id: u64) -> String {
    let mut chunks = Vec::new();
    loop {
        let mut part = (id & 0x7FFF) as u16;
        id >>= 15;
        if id > 0 {
            part |= 0x8000; // 최상위 1 비트: 다음 글자와 계속 연결됨
        }
        chunks.push(encode_aje_token(part));
        if id == 0 {
            break;
        }
    }
    chunks.into_iter().collect()
}

/// [가변 길이 디코더] AI정음 스트림에서 1~3글자 조합을 읽어 u64 ID 리스트로 복원
pub fn decode_var_ids(input: &str) -> Result<Vec<u64>, AjeError> {
    let mut ids = Vec::new();
    let mut current_id: u64 = 0;
    let mut shift = 0;
    let mut in_sequence = false;

    for (idx, c) in input.chars().enumerate() {
        let token = decode_aje_char(c, idx)?;
        let payload = (token & 0x7FFF) as u64;
        current_id |= payload << shift;

        if (token & 0x8000) == 0 {
            ids.push(current_id);
            current_id = 0;
            shift = 0;
            in_sequence = false;
        } else {
            shift += 15;
            in_sequence = true;
        }
    }

    if in_sequence {
        return Err(AjeError::IncompleteSequence);
    }

    Ok(ids)
}

pub fn encode_tokens(tokens: &[u64]) -> String {
    tokens.iter().map(|&t| encode_var_id(t)).collect()
}

pub fn encode_bytes(bytes: &[u8]) -> Result<String, AjeError> {
    if bytes.len() % 2 != 0 {
        return Err(AjeError::OddByteLength(bytes.len()));
    }
    let mut out = String::with_capacity((bytes.len() / 2) * 4);
    for chunk in bytes.chunks_exact(2) {
        let arr: [u8; 2] = chunk.try_into().unwrap();
        out.push(encode_aje_token(u16::from_be_bytes(arr)));
    }
    Ok(out)
}

fn print_usage() {
    eprintln!(
        "사용법 (공통 옵션: --dict <사전파일.json>, --json-limit <개수> [생략 시 전체 저장]):"
    );
    eprintln!(
        "  aje-cli learn-dict <lean_소스폴더> [최대개수]  (생략 시 전체 식별자를 1~3글자 가변 조합으로 모두 학습)"
    );
    eprintln!("  aje-cli compile <입력_lean> <출력_aje>");
    eprintln!("  aje-cli decompile <입력_aje> [출력_lean]");
    eprintln!("  aje-cli verify <입력_aje>");
    eprintln!("  aje-cli encode <입력_바이너리> <출력_aje>");
    eprintln!("  aje-cli decode <입력_aje> <출력_바이너리>");
}

fn run() -> Result<(), AjeError> {
    let raw_args: Vec<String> = env::args().skip(1).collect();
    let mut dict_path = DEFAULT_DICT_PATH.to_string();
    let mut json_limit: Option<usize> = None;
    let mut args = Vec::new();

    let mut i = 0;
    while i < raw_args.len() {
        match raw_args[i].as_str() {
            "--dict" => {
                let path = raw_args.get(i + 1).ok_or_else(|| {
                    AjeError::InvalidArgs("--dict 뒤에 경로를 입력하세요.".into())
                })?;
                dict_path = path.clone();
                i += 2;
            }
            "--json-limit" => {
                let val = raw_args
                    .get(i + 1)
                    .and_then(|s| s.parse::<usize>().ok())
                    .ok_or_else(|| {
                        AjeError::InvalidArgs("--json-limit 뒤에 숫자를 입력하세요.".into())
                    })?;
                json_limit = if val > 0 { Some(val) } else { None };
                i += 2;
            }
            other => {
                args.push(other.to_string());
                i += 1;
            }
        }
    }

    let cmd = match args.first() {
        Some(c) => c.as_str(),
        None => {
            print_usage();
            return Err(AjeError::InvalidArgs("명령어를 입력하세요.".into()));
        }
    };

    match cmd {
        "learn-dict" => {
            let src_dir = args.get(1).map(|s| s.as_str()).unwrap_or("./lean_src");
            let max_limit: Option<usize> = args.get(2).and_then(|s| s.parse().ok());

            let (scanned_files, unique_found, added_entries) =
                learner::learn_and_update_dict(src_dir, &dict_path, max_limit, json_limit)
                    .map_err(AjeError::ProcessError)?;

            println!(
                "사전 전체 학습 완료: '{}' 하위 {}개 .lean 파일에서 고유 식별자 {}개 발견 -> {}개 추가 등록 완료 ('{}')",
                src_dir, scanned_files, unique_found, added_entries, dict_path
            );
        }
        "compile" => {
            let in_path = args
                .get(1)
                .ok_or_else(|| AjeError::InvalidArgs("입력 Lean 파일 누락".into()))?;
            let out_path = args
                .get(2)
                .ok_or_else(|| AjeError::InvalidArgs("출력 AJE 파일 누락".into()))?;
            let dictionary =
                MathlibDict::load_from_file(&dict_path).map_err(AjeError::ProcessError)?;

            let lean_src = fs::read_to_string(in_path)?;
            let tokens =
                compiler::compile_lean4(&lean_src, &dictionary).map_err(AjeError::ProcessError)?;
            let encoded = encode_tokens(&tokens);

            fs::File::create(out_path)?.write_all(encoded.as_bytes())?;
            println!(
                "컴파일 완료: {} bytes (Lean 4) -> {} 토큰 / {} 글자 (AI정음, '{}')",
                lean_src.len(),
                tokens.len(),
                encoded.chars().count(),
                out_path
            );
        }
        "decompile" => {
            let in_path = args
                .get(1)
                .ok_or_else(|| AjeError::InvalidArgs("입력 AJE 파일 누락".into()))?;
            let dictionary =
                MathlibDict::load_from_file(&dict_path).map_err(AjeError::ProcessError)?;

            let content = fs::read_to_string(in_path)?;
            let tokens = decode_var_ids(content.trim_end_matches(['\r', '\n']))?;
            let result = decompiler::decompile_tokens(&tokens, &dictionary);

            println!("{}", result.korean_explanation);
            println!("{}", result.lean4_code);

            if let Some(out_path) = args.get(2) {
                fs::File::create(out_path)?.write_all(result.lean4_code.as_bytes())?;
                println!("복원된 Lean 4 코드가 '{}' 파일로 저장되었습니다.", out_path);
            }
        }
        "verify" => {
            let in_path = args
                .get(1)
                .ok_or_else(|| AjeError::InvalidArgs("입력 파일 누락".into()))?;
            let content = fs::read_to_string(in_path)?;
            let trimmed = content.trim_end_matches(['\r', '\n']);
            let ids = decode_var_ids(trimmed)?;
            println!(
                "검증 통과: 총 {} 글리프 -> {}개 가변 길이 토큰 복원",
                trimmed.chars().count(),
                ids.len()
            );
            for (i, id) in ids.iter().enumerate() {
                let glyphs = encode_var_id(*id).chars().count();
                println!("  [{:04}] Token ID: 0x{:X} ({}글자 조합)", i, id, glyphs);
            }
        }

        "verify-lean" => {
            let target_path = args
                .get(1)
                .map(|s| s.as_str())
                .unwrap_or("./restored_lean_src");
            let paranoid = args.iter().any(|a| a == "--paranoid");

            verifier::run_lean_verification(target_path, paranoid)
                .map_err(AjeError::ProcessError)?;
        }

        "encode" => {
            let in_path = args
                .get(1)
                .ok_or_else(|| AjeError::InvalidArgs("입력 파일 누락".into()))?;
            let out_path = args
                .get(2)
                .ok_or_else(|| AjeError::InvalidArgs("출력 파일 누락".into()))?;
            let mut buf = Vec::new();
            fs::File::open(in_path)?.read_to_end(&mut buf)?;
            let encoded = encode_bytes(&buf)?;
            fs::File::create(out_path)?.write_all(encoded.as_bytes())?;
            println!(
                "raw bytes 인코딩 완료: {} bytes -> {} 글자",
                buf.len(),
                encoded.chars().count()
            );
        }
        "decode" => {
            let in_path = args
                .get(1)
                .ok_or_else(|| AjeError::InvalidArgs("입력 파일 누락".into()))?;
            let out_path = args
                .get(2)
                .ok_or_else(|| AjeError::InvalidArgs("출력 파일 누락".into()))?;
            let content = fs::read_to_string(in_path)?;
            let trimmed = content.trim_end_matches(['\r', '\n']);
            let mut bytes = Vec::with_capacity(trimmed.chars().count() * 2);
            for (idx, c) in trimmed.chars().enumerate() {
                let raw_u16 = decode_aje_char(c, idx)?;
                bytes.extend_from_slice(&raw_u16.to_be_bytes());
            }
            fs::File::create(out_path)?.write_all(&bytes)?;
            println!("raw bytes 디코딩 완료: {} bytes 복원", bytes.len());
        }

        "compile-project" => {
            let src_dir = args.get(1).map(|s| s.as_str()).unwrap_or("./lean_src");
            let out_path = args
                .get(2)
                .map(|s| s.as_str())
                .unwrap_or("project_full.aje");
            let dictionary =
                MathlibDict::load_from_file(&dict_path).map_err(AjeError::ProcessError)?;

            let (tokens, file_count, total_bytes) =
                compiler::compile_project(src_dir, &dictionary).map_err(AjeError::ProcessError)?;
            let encoded = encode_tokens(&tokens);

            fs::File::create(out_path)?.write_all(encoded.as_bytes())?;
            println!(
                "프로젝트 통합 압축 완료: '{}' ({}개 파일, {} bytes) -> {} 토큰 / {} 글자 ('{}' 저장됨)",
                src_dir,
                file_count,
                total_bytes,
                tokens.len(),
                encoded.chars().count(),
                out_path
            );
        }

        "decompile-project" => {
            let aje_path = args.get(1).ok_or_else(|| {
                AjeError::InvalidArgs("decompile-project: .aje 프로젝트 파일을 입력하세요.".into())
            })?;
            let out_dir = args
                .get(2)
                .map(|s| s.as_str())
                .unwrap_or("./restored_project");

            let dictionary =
                MathlibDict::load_from_file(&dict_path).map_err(AjeError::ProcessError)?;
            let content = fs::read_to_string(aje_path)?;
            let tokens = decode_var_ids(content.trim_end_matches(['\r', '\n']))?;

            let restored_files =
                decompiler::decompile_project_to_dir(&tokens, &dictionary, out_dir)
                    .map_err(AjeError::ProcessError)?;

            println!(
                "프로젝트 트리 복원 완료: '{}' -> '{}' 폴더 아래 {}개 .lean 파일 및 한국어 해설(.ko.md, README_KO.md) 생성됨",
                aje_path, out_dir, restored_files
            );
        }

        "stats" => {
            let aje_path = args.get(1).ok_or_else(|| {
                AjeError::InvalidArgs("분석할 .aje 파일 경로를 입력하세요.".into())
            })?;
            let orig_path = args.get(2).map(|s| s.as_str());
            let top_n: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(15);

            let dictionary =
                MathlibDict::load_from_file(&dict_path).map_err(AjeError::ProcessError)?;
            let content = fs::read_to_string(aje_path)?;
            let trimmed = content.trim_end_matches(['\r', '\n']);
            let tokens = decode_var_ids(trimmed)?;

            stats::print_aje_stats(trimmed, &tokens, &dictionary, orig_path, top_n)
                .map_err(AjeError::ProcessError)?;
        }

        "search-dict" => {
            let query = args.get(1).ok_or_else(|| {
                AjeError::InvalidArgs(
                    "검색할 정리 이름 또는 16진수 토큰 ID(예: 0x1A3B)를 입력하세요.".into(),
                )
            })?;
            let dictionary =
                MathlibDict::load_from_file(&dict_path).map_err(AjeError::ProcessError)?;

            let q_lower = query.to_lowercase();
            let clean_hex = q_lower.trim_start_matches("0x");
            let target_token = u64::from_str_radix(clean_hex, 16).ok();

            let mut found = 0usize;
            println!("=== [사전 검색 결과: '{}'] ===", query);
            for e in &dictionary.entries {
                let is_match = target_token == Some(e.token)
                    || e.lean_pattern.to_lowercase().contains(&q_lower);

                if is_match {
                    println!(
                        "[Order: {:>7}] Token: 0x{:05X} | Freq: {:>6}회 | 패턴: {} -> {}",
                        e.order, e.token, e.freq, e.lean_pattern, e.ko_desc
                    );
                    found += 1;
                    if found >= 30 {
                        println!("... (검색 결과가 많아 상위 30개만 표시합니다)");
                        break;
                    }
                }
            }

            if found == 0 {
                println!("일치하는 항목이 없습니다.");
            }
        }

        "edit-dict" => {
            let token_arg = args.get(1).ok_or_else(|| {
                AjeError::InvalidArgs("수정할 토큰 ID(예: 0x0001)를 입력하세요.".into())
            })?;
            let new_desc = args.get(2).ok_or_else(|| {
                AjeError::InvalidArgs("새로운 한국어 해설 문장을 따옴표로 감싸 입력하세요.".into())
            })?;

            let clean_hex = token_arg
                .trim()
                .trim_start_matches("0x")
                .trim_start_matches("0X");
            let target_token = u64::from_str_radix(clean_hex, 16).map_err(|_| {
                AjeError::InvalidArgs(format!("잘못된 16진수 토큰 형식: {}", token_arg))
            })?;

            let mut dictionary =
                MathlibDict::load_from_file(&dict_path).map_err(AjeError::ProcessError)?;

            let mut updated_pattern = None;
            let mut old_desc = String::new();

            if let Some(entry) = dictionary.by_token.get_mut(&target_token) {
                old_desc = entry.ko_desc.clone();
                entry.ko_desc = new_desc.clone();
                updated_pattern = Some(entry.lean_pattern.clone());
            }

            if let Some(pattern) = updated_pattern {
                if let Some(vec_entry) = dictionary
                    .entries
                    .iter_mut()
                    .find(|e| e.token == target_token)
                {
                    vec_entry.ko_desc = new_desc.clone();
                }

                // 1. 전체 바이너리 사전(.bin) 즉시 업데이트
                let bin_path = format!("{}.bin", dict_path);
                dictionary
                    .save_binary(&bin_path)
                    .map_err(AjeError::ProcessError)?;

                // 2. 상위 3,000개 JSON 파일 동기화
                learner::save_top_json(&dictionary.entries, &dict_path, 3000)
                    .map_err(AjeError::ProcessError)?;

                println!("==================================================================");
                println!(
                    "▶ 사전 해설 수정 완료: Token 0x{:X} (`{}`)",
                    target_token, pattern
                );
                println!("  - 기존 해설: {}", old_desc);
                println!("  - 변경 해설: {}", new_desc);
                println!("==================================================================");
            } else {
                return Err(AjeError::ProcessError(format!(
                    "사전에서 토큰 0x{:X}를 찾을 수 없습니다. `search-dict`로 토큰 ID를 먼저 확인하세요.",
                    target_token
                )));
            }
        }

        "split-dict" => {
            let out_dir = args.get(1).map(|s| s.as_str()).unwrap_or("./dict_dir");
            let chunk_size: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(500);

            let created_files = collab::split_dictionary(&dict_path, out_dir, chunk_size)
                .map_err(AjeError::ProcessError)?;

            println!(
                "사전 분할 완료: '{}' -> '{}' 폴더에 {}개 파일(파일당 최대 {}개) 생성됨",
                dict_path, out_dir, created_files, chunk_size
            );
        }
        "merge-dict" => {
            let dict_dir = args.get(1).map(|s| s.as_str()).unwrap_or("./dict_dir");
            let total_merged = collab::merge_dictionary(dict_dir, &dict_path, json_limit)
                .map_err(AjeError::ProcessError)?;

            println!(
                "사전 통합 완료: '{}' 내 로컬 분할 파일 및 원격 URL 항목 총 {}개가 '{}' 및 바이너리 캐시로 병합되었습니다.",
                dict_dir, total_merged, dict_path
            );
        }

        "export-dataset" => {
            let src_dir = args.get(1).map(|s| s.as_str()).unwrap_or("./lean_src");
            let out_jsonl = args.get(2).map(|s| s.as_str()).unwrap_or("train.jsonl");
            let max_modules: Option<usize> = args.get(3).and_then(|s| s.parse().ok());

            let dictionary =
                MathlibDict::load_from_file(&dict_path).map_err(AjeError::ProcessError)?;
            let (modules, samples) =
                dataset::export_training_dataset(src_dir, &dictionary, out_jsonl, max_modules)
                    .map_err(AjeError::ProcessError)?;

            println!("==================================================================");
            println!("▶ AI 파인튜닝 데이터셋 추출 완료: '{}'", out_jsonl);
            println!("  - 추출된 증명 모듈 수 : {} 개", modules);
            println!(
                "  - 생성된 학습 샘플 수 : {} 개 (압축 / 코드 복원 / 한국어 해설 3종 쌍)",
                samples
            );
            println!("==================================================================");
        }

        other => {
            print_usage();
            return Err(AjeError::InvalidArgs(format!(
                "알 수 없는 명령어: {}",
                other
            )));
        }
    }

    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("[오류] {}", e);
        process::exit(1);
    }
}
