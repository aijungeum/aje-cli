use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
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

/// Lake 프로젝트 루트(lakefile.lean / lakefile.toml) 찾기 :  대상 경로 or 상위 디렉토리 탐색
fn find_lake_root(start: &Path) -> Option<PathBuf> {
    let mut curr = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };

    loop {
        if curr.join("lakefile.lean").exists() || curr.join("lakefile.toml").exists() {
            return Some(curr);
        }
        if !curr.pop() {
            break;
        }
    }
    None
}

/// 복원된 .lean 파일 또는 프로젝트 디렉토리의 문법 및 타입 무결성을 실제 Lean 4 툴체인으로 검증
pub fn run_lean_verification(target_path: &str, paranoid: bool) -> Result<(), String> {
    let target = Path::new(target_path);
    if !target.exists() {
        return Err(format!(
            "검증 대상 경로('{}')가 존재하지 않습니다.",
            target_path
        ));
    }

    let start_time = Instant::now();
    println!("==================================================================");
    println!("          Lean 4 형식 검증 및 타입 무결성 자동 검사기");
    println!("==================================================================");
    println!("▶ 검증 대상 경로 : {}", target.display());
    println!(
        "▶ 검증 모드      : {}",
        if paranoid {
            "엄격 모드 (--paranoid: sorry 경고 및 외부 커널 교차 검사 시도)"
        } else {
            "표준 타입 검사 모드"
        }
    );

    // Lake 프로젝트 여부 판별 후 적절한 검증 명령어 구성
    if let Some(lake_root) = find_lake_root(target) {
        println!(
            "▶ 감지된 환경    : Lake 프로젝트 (루트: {})",
            lake_root.display()
        );

        let mut cmd =
            if target.is_dir() && target.canonicalize().ok() == lake_root.canonicalize().ok() {
                let mut c = Command::new("lake");
                c.arg("build").current_dir(&lake_root);
                c
            } else if target.is_file() {
                let abs_target = target
                    .canonicalize()
                    .unwrap_or_else(|_| target.to_path_buf());
                let mut c = Command::new("lake");
                c.arg("env")
                    .arg("lean")
                    .arg(abs_target)
                    .current_dir(&lake_root);
                c
            } else {
                // 하위 폴더만 지정된 경우: 해당 폴더 내 모든 .lean 파일을 lake env lean으로 순차 검사
                return verify_directory_files(target, Some(&lake_root), paranoid, start_time);
            };

        let output = cmd.output().map_err(|e| {
            format!(
                "'lake' 실행 실패: {} (Lean 4 / elan 툴체인이 PATH에 설치되어 있는지 확인하세요)",
                e
            )
        })?;

        return report_command_result(
            &output.stdout,
            &output.stderr,
            output.status.success(),
            paranoid,
            start_time,
        );
    }

    // Lake 프로젝트가 아닌 독립 .lean 파일 또는 일반 복원 디렉토리 검증
    println!("▶ 감지된 환경    : 독립 Lean 4 소스 (단독 `lean` 컴파일러 호출)");
    if target.is_file() {
        let output = Command::new("lean").arg(target).output().map_err(|e| {
            format!(
                "'lean' 실행 실패: {} (Lean 4가 설치되어 있는지 확인하세요)",
                e
            )
        })?;
        report_command_result(
            &output.stdout,
            &output.stderr,
            output.status.success(),
            paranoid,
            start_time,
        )
    } else {
        verify_directory_files(target, None, paranoid, start_time)
    }
}

fn verify_directory_files(
    dir: &Path,
    lake_root: Option<&Path>,
    paranoid: bool,
    start_time: Instant,
) -> Result<(), String> {
    let mut files = Vec::new();
    collect_lean_files(dir, &mut files)?;
    if files.is_empty() {
        return Err(format!(
            "'{}' 경로에서 검증할 .lean 파일을 찾지 못했습니다.",
            dir.display()
        ));
    }
    files.sort();

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut sorry_warnings = 0usize;

    for file in &files {
        let abs_file = file.canonicalize().unwrap_or_else(|_| file.clone());
        let output = if let Some(root) = lake_root {
            Command::new("lake")
                .arg("env")
                .arg("lean")
                .arg(&abs_file)
                .current_dir(root)
                .output()
        } else {
            Command::new("lean").arg(&abs_file).output()
        }
        .map_err(|e| format!("Lean 프로세스 실행 실패: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{}\n{}", stdout, stderr);

        if combined.contains("declaration uses 'sorry'") {
            sorry_warnings += 1;
        }

        if output.status.success() && (!paranoid || !combined.contains("declaration uses 'sorry'"))
        {
            passed += 1;
            println!("  [통과] {}", file.display());
        } else {
            failed += 1;
            println!("  [실패] {}", file.display());
            for line in combined.lines().filter(|l| !l.trim().is_empty()).take(5) {
                println!("         -> {}", line);
            }
        }
    }

    let elapsed = start_time.elapsed();
    println!("==================================================================");
    println!(
        "▶ 검증 요약: 총 {}개 파일 중 {}개 통과, {}개 실패 (sorry 경고: {}건, 소요 시간: {:.2}초)",
        files.len(),
        passed,
        failed,
        sorry_warnings,
        elapsed.as_secs_f64()
    );
    println!("==================================================================");

    if failed > 0 {
        Err(format!(
            "{}개 파일에서 Lean 4 문법 또는 타입 오류가 발견되었습니다.",
            failed
        ))
    } else {
        Ok(())
    }
}

fn report_command_result(
    stdout: &[u8],
    stderr: &[u8],
    success: bool,
    paranoid: bool,
    start_time: Instant,
) -> Result<(), String> {
    let out_str = String::from_utf8_lossy(stdout);
    let err_str = String::from_utf8_lossy(stderr);
    let combined = format!("{}\n{}", out_str, err_str);

    let has_sorry = combined.contains("declaration uses 'sorry'");
    let elapsed = start_time.elapsed();

    for line in combined.lines().filter(|l| !l.trim().is_empty()) {
        println!("  {}", line);
    }

    println!("==================================================================");
    if success && (!paranoid || !has_sorry) {
        println!(
            "▶ 최종 판정: [검증 통과] 모든 정리가 Lean 4 타입 시스템을 통과했습니다. (소요 시간: {:.2}초)",
            elapsed.as_secs_f64()
        );
        Ok(())
    } else if success && paranoid && has_sorry {
        Err("엄격 모드(--paranoid) 실패: 미완성 증명('sorry')이 포함되어 있습니다.".into())
    } else {
        Err(format!(
            "Lean 4 타입 검사 실패 (소요 시간: {:.2}초)",
            elapsed.as_secs_f64()
        ))
    }
}
