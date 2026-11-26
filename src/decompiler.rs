use crate::compiler::MODULE_HEADER_TOKEN;
use crate::dict::MathlibDict;
use std::fmt::Write;
use std::fs;
use std::path::{Component, Path, PathBuf};

pub struct DecompiledOutput {
    pub korean_explanation: String,
    pub lean4_code: String,
}

/// 파서 : 시작신호(MODULE_HEADER_TOKEN) 만나면 본문(상대경로, 토큰 수) 추출
/// 토큰 : PUA 유니코드 글리프에서 숫자(u64)로 디코딩된 토큰 ID 배열
pub fn parse_module_header(tokens: &[u64], start_idx: usize) -> Result<(String, usize), String> {
    let byte_len = *tokens
        .get(start_idx + 1)
        .ok_or_else(|| "모듈 헤더의 경로 길이 토큰이 누락되었습니다.".to_string())?
        as usize;

    if byte_len > 4096 || start_idx + 2 + byte_len > tokens.len() {
        return Err("유효하지 않은 헤더 길이".into());
    }

    let mut raw_bytes = Vec::with_capacity(byte_len);
    for b_idx in 0..byte_len {
        let val = *tokens
            .get(start_idx + 2 + b_idx)
            .ok_or_else(|| "모듈 헤더의 경로 데이터 토큰이 불완전합니다.".to_string())?;
        if val > 0xFF {
            return Err("경로 바이트 범위 초과".into());
        }
        raw_bytes.push(val as u8);
    }

    let rel_path = String::from_utf8(raw_bytes)
        .map_err(|_| "모듈 헤더의 파일 경로 UTF-8 디코딩에 실패했습니다.".to_string())?;

    Ok((rel_path, 2 + byte_len))
}

/// decompile: 토큰 스트림을, 한국어 해설과 Lean 4 코드로 복원
pub fn decompile_tokens(tokens: &[u64], dict: &MathlibDict) -> DecompiledOutput {
    let mut ko = String::new();
    let mut lean = String::new();
    let mut step_num = 1usize;
    let mut i = 0usize;

    writeln!(ko, "=== [AI정음 역변환: 한국어 직관 해설] ===").unwrap();
    writeln!(lean, "-- [AI정음 역변환: Lean 4 형식 검증 코드]").unwrap();
    writeln!(lean, "import Mathlib.NumberTheory.LSeries.RiemannZeta\n").unwrap();

    while i < tokens.len() {
        let token = tokens[i];
        // 헤더가 있으면 주석으로 파일명 표시
        if token == MODULE_HEADER_TOKEN {
            if let Ok((rel_path, consumed)) = parse_module_header(tokens, i) {
                step_num = 1;
                writeln!(ko, "\n▶ [파일 모듈: {}]", rel_path).unwrap();
                writeln!(lean, "\n-- === File: {} ===", rel_path).unwrap();
                i += consumed;
                continue;
            }
        }

        if let Some(entry) = dict.by_token.get(&token) {
            writeln!(ko, "{}. {}", step_num, entry.ko_desc).unwrap();
            write!(lean, "{}", entry.lean_emit).unwrap();
        } else {
            writeln!(
                ko,
                "{}. [미등록 토큰] ID `0x{:X}` 를 통과시킵니다.",
                step_num, token
            )
            .unwrap();
            writeln!(lean, "  -- unknown token ID: 0x{:X}", token).unwrap();
        }
        step_num += 1;
        i += 1;
    }

    DecompiledOutput {
        korean_explanation: ko,
        lean4_code: lean,
    }
}

/// 상대 경로 결합: 상위 디렉토리 .. 이탈을 차단
fn safe_join(base_dir: &Path, rel_path: &str) -> Result<PathBuf, String> {
    let mut clean_rel = PathBuf::new();
    for comp in Path::new(rel_path).components() {
        match comp {
            Component::Normal(p) => clean_rel.push(p),
            Component::CurDir => {}
            _ => {
                return Err(format!(
                    "안전하지 않은 경로가 감지되어 차단되었습니다: '{}'",
                    rel_path
                ));
            }
        }
    }
    if clean_rel.as_os_str().is_empty() {
        return Err("복원할 파일 경로가 비어 있습니다.".into());
    }
    Ok(base_dir.join(clean_rel))
}

/// 통합 .aje 토큰 스트림을, 원래의 폴더 구조와 개별 .lean 및 .ko.md 해설 파일들로 복원
pub fn decompile_project_to_dir(
    tokens: &[u64],
    dict: &MathlibDict,
    out_dir: &str,
) -> Result<usize, String> {
    let base_path = Path::new(out_dir);
    fs::create_dir_all(base_path)
        .map_err(|e| format!("출력 디렉토리('{}') 생성 실패: {}", out_dir, e))?;

    struct ModuleBlock {
        rel_path: String,
        body_tokens: Vec<u64>,
    }

    let mut modules: Vec<ModuleBlock> = Vec::new();
    let mut i = 0usize;

    while i < tokens.len() {
        if tokens[i] == MODULE_HEADER_TOKEN {
            let (rel_path, consumed) = parse_module_header(tokens, i)?;
            modules.push(ModuleBlock {
                rel_path,
                body_tokens: Vec::new(),
            });
            i += consumed;
        } else {
            if modules.is_empty() {
                // 헤더 없는 .aje 파일 처리
                modules.push(ModuleBlock {
                    rel_path: "RestoredMain.lean".to_string(),
                    body_tokens: Vec::new(),
                });
            }
            if let Some(last) = modules.last_mut() {
                last.body_tokens.push(tokens[i]);
            }
            i += 1;
        }
    }

    if modules.is_empty() {
        return Err("복원할 모듈 데이터가 없습니다.".into());
    }

    let mut restored_count = 0usize;
    let mut index_md = String::from("# AI정음(AiJungEum) 프로젝트 역변환 해설 목차\n\n");

    for m in &modules {
        let target_lean_path = safe_join(base_path, &m.rel_path)?;
        if let Some(parent) = target_lean_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("하위 폴더('{}') 생성 실패: {}", parent.display(), e))?;
        }

        let mut lean_code = String::new();
        let mut ko_md = String::new();

        writeln!(lean_code, "-- [AI정음 자동 복원 파일: {}]", m.rel_path).unwrap();
        writeln!(
            lean_code,
            "import Mathlib.NumberTheory.LSeries.RiemannZeta\n"
        )
        .unwrap();

        writeln!(ko_md, "# 증명 해설: `{}`\n", m.rel_path).unwrap();

        for (step, &tok) in m.body_tokens.iter().enumerate() {
            let step_num = step + 1;
            if let Some(entry) = dict.by_token.get(&tok) {
                write!(lean_code, "{}", entry.lean_emit).unwrap();
                writeln!(ko_md, "{}. {}", step_num, entry.ko_desc).unwrap();
            } else {
                writeln!(lean_code, "  -- unknown token ID: 0x{:X}", tok).unwrap();
                writeln!(ko_md, "{}. [미등록 토큰] ID `0x{:X}`", step_num, tok).unwrap();
            }
        }

        // 개별 .lean 파일 저장
        fs::write(&target_lean_path, lean_code)
            .map_err(|e| format!("파일('{}') 저장 실패: {}", target_lean_path.display(), e))?;

        // 같은 위치에 한국어 해설 마크다운(.ko.md) 동시 생성
        let ko_path_str = format!("{}.ko.md", target_lean_path.display());
        fs::write(&ko_path_str, ko_md)
            .map_err(|e| format!("해설 파일('{}') 저장 실패: {}", ko_path_str, e))?;

        writeln!(
            index_md,
            "- '{}' ({}개 핵심 논리 단계) → [`{}.ko.md`](./{}.ko.md)",
            m.rel_path,
            m.body_tokens.len(),
            m.rel_path,
            m.rel_path
        )
        .unwrap();

        restored_count += 1;
    }

    // 루트에 전체 프로젝트 해설 목차(README_KO.md) 생성
    let index_path = base_path.join("README_KO.md");
    fs::write(&index_path, index_md)
        .map_err(|e| format!("목차 파일('{}') 저장 실패: {}", index_path.display(), e))?;

    Ok(restored_count)
}
