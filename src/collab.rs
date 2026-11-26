use crate::dict::{DictEntry, MathlibDict};
use crate::learner::{save_dict_json, serialize_dict_to_json};
use std::collections::HashMap;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct ConflictRecord {
    token: u64,
    lean_pattern: String,
    source_a: String,
    desc_a: String,
    source_b: String,
    desc_b: String,
}

/// 미수정 기본 템플릿 문구인지 판별
fn is_default_template(desc: &str) -> bool {
    let t = desc.trim();
    t.starts_with("[자동 학습") || t.starts_with("[핵심 선언")
}

/// 사전을 chunk_size 단위로 분할하여 out_dir/mathlib_dict_001.json 형태로 저장
pub fn split_dictionary(
    dict_path: &str,
    out_dir: &str,
    chunk_size: usize,
) -> Result<usize, String> {
    if chunk_size == 0 {
        return Err("분할 단위(chunk_size)는 1 이상이어야 합니다.".into());
    }

    let dict = MathlibDict::load_from_file(dict_path)?;
    let dir_path = Path::new(out_dir);
    fs::create_dir_all(dir_path)
        .map_err(|e| format!("출력 폴더('{}') 생성 실패: {}", out_dir, e))?;

    let mut file_count = 0usize;
    for (idx, chunk) in dict.entries.chunks(chunk_size).enumerate() {
        let part_num = idx + 1;
        let file_name = format!("mathlib_dict_{:03}.json", part_num);
        let file_path = dir_path.join(&file_name);

        let json_str = serialize_dict_to_json(chunk);
        fs::write(&file_path, json_str)
            .map_err(|e| format!("분할 파일('{}') 저장 실패: {}", file_path.display(), e))?;
        file_count += 1;
    }

    Ok(file_count)
}

/// curl을 이용해 원격 URL의 JSON 문자열을 다운로드
fn fetch_url_content(url: &str) -> Result<String, String> {
    let output = Command::new("curl")
        .arg("-fsSL")
        .arg("--connect-timeout")
        .arg("10")
        .arg(url)
        .output()
        .map_err(|e| format!("'curl' 실행 실패 ({}): {}", url, e))?;

    if !output.status.success() {
        let err_msg = String::from_utf8_lossy(&output.stderr);
        return Err(format!("URL 다운로드 실패 ({}): {}", url, err_msg.trim()));
    }

    String::from_utf8(output.stdout).map_err(|_| {
        format!(
            "다운로드된 데이터가 올바른 UTF-8 텍스트가 아닙니다: {}",
            url
        )
    })
}

/// 머지 충돌: 항목 삽입 시 기존 항목과 ko_desc 충돌 여부를 검사하고 병합
fn insert_with_conflict_check(
    merged_map: &mut HashMap<u64, (DictEntry, String)>,
    conflicts: &mut Vec<ConflictRecord>,
    incoming: DictEntry,
    source_name: &str,
) {
    if let Some((existing, existing_src)) = merged_map.get(&incoming.token) {
        let old_desc = existing.ko_desc.trim();
        let new_desc = incoming.ko_desc.trim();

        if old_desc == new_desc {
            // 내용이 완전히 같으면 출처만 최신으로 갱신
            merged_map.insert(incoming.token, (incoming, source_name.to_string()));
            return;
        }

        let old_is_default = is_default_template(old_desc);
        let new_is_default = is_default_template(new_desc);

        if old_is_default && !new_is_default {
            // 기존은 미수정 템플릿이고 새 항목은 사람이 수정한 경우 -> 충돌 없이 새 항목 채택
            merged_map.insert(incoming.token, (incoming, source_name.to_string()));
            return;
        }

        if !old_is_default && new_is_default {
            // 기존은 사람이 수정했고 새 항목은 미수정 템플릿인 경우 -> 기존 수정본 보호
            return;
        }

        // 두 작업자 모두 서로 다르게 수정한 경우 -> 충돌 기록 후 최신 항목 반영
        conflicts.push(ConflictRecord {
            token: incoming.token,
            lean_pattern: incoming.lean_pattern.clone(),
            source_a: existing_src.clone(),
            desc_a: existing.ko_desc.clone(),
            source_b: source_name.to_string(),
            desc_b: incoming.ko_desc.clone(),
        });
    }

    merged_map.insert(incoming.token, (incoming, source_name.to_string()));
}

/// ./dict_dir 내 분할 JSON 파일들과 mathlib_dict.txt 내 URL들을 취합하고 충돌 리포트 생성
pub fn merge_dictionary(
    dict_dir: &str,
    out_dict_path: &str,
    json_limit: Option<usize>,
) -> Result<usize, String> {
    let dir_path = Path::new(dict_dir);
    if !dir_path.exists() {
        return Err(format!(
            "사전 작업 폴더('{}')가 존재하지 않습니다.",
            dict_dir
        ));
    }

    let mut merged_map: HashMap<u64, (DictEntry, String)> = HashMap::new();
    let mut conflicts: Vec<ConflictRecord> = Vec::new();

    // 로컬 분할 파일(mathlib_dict_*.json) 수집 및 이름순 정렬
    let mut part_files: Vec<PathBuf> = Vec::new();
    let entries =
        fs::read_dir(dir_path).map_err(|e| format!("폴더('{}') 읽기 실패: {}", dict_dir, e))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(fname) = path.file_name().and_then(|n| n.to_str()) {
                if fname.starts_with("mathlib_dict_") && fname.ends_with(".json") {
                    part_files.push(path);
                }
            }
        }
    }
    part_files.sort();

    for file in &part_files {
        let raw = fs::read_to_string(file)
            .map_err(|e| format!("분할 파일('{}') 읽기 실패: {}", file.display(), e))?;
        let part_dict = MathlibDict::parse_json(&raw)
            .map_err(|e| format!("분할 파일('{}') 파싱 오류: {}", file.display(), e))?;

        let src_label = file.display().to_string();
        println!(
            "  -> [로컬 병합] {} ({}개 항목 로드)",
            src_label,
            part_dict.entries.len()
        );
        for item in part_dict.entries {
            insert_with_conflict_check(&mut merged_map, &mut conflicts, item, &src_label);
        }
    }

    // ./dict_dir/mathlib_dict.txt 원격 URL 목록 확인 및 다운로드
    let txt_path = dir_path.join("mathlib_dict.txt");
    if txt_path.exists() {
        let txt_raw = fs::read_to_string(&txt_path)
            .map_err(|e| format!("URL 목록 파일('{}') 읽기 실패: {}", txt_path.display(), e))?;

        let first_line = txt_raw.lines().next().unwrap_or("").trim();
        if first_line.eq_ignore_ascii_case("[download:off]") {
            println!(
                "  -> [원격 건너뜀] '{}' 상단에 [download:off]가 설정되어 있습니다.",
                txt_path.display()
            );
        } else {
            let mut urls = Vec::new();
            for line in txt_raw.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('[') {
                    continue;
                }
                if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                    urls.push(trimmed.to_string());
                }
            }

            if !urls.is_empty() {
                let mut downloaded_any = false;
                for url in &urls {
                    println!("  -> [원격 다운로드] {} 요청 중...", url);
                    let remote_json = fetch_url_content(url)?;
                    let remote_dict = MathlibDict::parse_json(&remote_json)
                        .map_err(|e| format!("원격 JSON 파싱 오류 ({}): {}", url, e))?;

                    println!("     => {}개 항목 병합 완료", remote_dict.entries.len());
                    for item in remote_dict.entries {
                        insert_with_conflict_check(&mut merged_map, &mut conflicts, item, url);
                    }
                    downloaded_any = true;
                }

                if downloaded_any {
                    let updated_txt = format!("[download:off]\n{}", txt_raw);
                    fs::write(&txt_path, updated_txt).map_err(|e| {
                        format!("'{}' 상태 업데이트 실패: {}", txt_path.display(), e)
                    })?;
                    println!(
                        "  -> [상태 기록] '{}' 상단에 [download:off]가 명시되었습니다.",
                        txt_path.display()
                    );
                }
            }
        }
    }

    if merged_map.is_empty() {
        return Err(format!(
            "'{}'에서 병합할 유효한 사전 데이터를 찾지 못했습니다.",
            dict_dir
        ));
    }

    // 충돌 리포트 출력 및 파일(merge_conflicts.md) 저장
    let report_path = dir_path.join("merge_conflicts.md");
    if conflicts.is_empty() {
        println!("  -> [충돌 검사] 한국어 해설(ko_desc) 충돌 0건 (깨끗하게 병합됨)");
        if report_path.exists() {
            let _ = fs::remove_file(&report_path);
        }
    } else {
        println!("\n==================================================================");
        println!(
            "  ⚠️  [경고] 동일 토큰 한국어 해설(ko_desc) 충돌 {}건 감지!",
            conflicts.len()
        );
        println!("==================================================================");

        let mut md = String::from("# AI정음 사전 병합 충돌 리포트\n\n");
        writeln!(
            md,
            "총 {}건의 동일 토큰에 대해 서로 다른 한국어 해설이 감지되었습니다.\n",
            conflicts.len()
        )
        .unwrap();
        writeln!(md, "> 기본 동작으로 '나중에 병합된 버전(B)'이 반영되었으며, 최종 확정은 아래 `edit-dict` 명령어로 수정할 수 있습니다.\n").unwrap();

        for (idx, c) in conflicts.iter().enumerate() {
            println!(
                "  {}. Token: 0x{:X} (`{}`)",
                idx + 1,
                c.token,
                c.lean_pattern
            );
            println!("     - [A] ({}) : {}", c.source_a, c.desc_a);
            println!(
                "     - [B] ({}) : {}  <-- 현재 반영됨",
                c.source_b, c.desc_b
            );

            writeln!(
                md,
                "### {}. Token `0x{:X}` (`{}`)",
                idx + 1,
                c.token,
                c.lean_pattern
            )
            .unwrap();
            writeln!(md, "- 버전 A (`{}`): {}", c.source_a, c.desc_a).unwrap();
            writeln!(
                md,
                "- 버전 B (`{}`): {} (현재 반영됨)",
                c.source_b, c.desc_b
            )
            .unwrap();
            writeln!(
                md,
                "- 수동 확정 명령어: `aje-cli edit-dict 0x{:X} \"<최종 해설>\"`\n",
                c.token
            )
            .unwrap();
        }
        println!("==================================================================");
        println!(
            "  -> 상세 충돌 내역이 '{}'에 저장되었습니다.\n",
            report_path.display()
        );

        fs::write(&report_path, md)
            .map_err(|e| format!("충돌 리포트('{}') 저장 실패: {}", report_path.display(), e))?;
    }

    // order 순 정렬 후 최종 JSON 및 바이너리 캐시(.bin) 저장
    let mut final_entries: Vec<DictEntry> = merged_map.into_values().map(|(e, _)| e).collect();
    final_entries.sort_by_key(|e| e.order);

    let total_count = final_entries.len();
    let final_dict = MathlibDict {
        by_token: final_entries.iter().map(|e| (e.token, e.clone())).collect(),
        entries: final_entries,
    };

    save_dict_json(&final_dict.entries, out_dict_path, json_limit)?;

    let bin_path = format!("{}.bin", out_dict_path);
    final_dict.save_binary(&bin_path)?;

    Ok(total_count)
}
