aje-cli 

version: 0.1.alpha 
author: <aijungeum.com> <aijungeum@gmail.com> 
license: MIT 
date: 2026.11.25


AI정음(AiJungEum)은 ASI to ASI, ASI to Human 소통을 위해 고안된 프로토콜 언어 입니다. 

리만가설(Riemann hypothesis) 증명 Lean코드를 ASI의 표현으로 규정하고, 
AI정음을 이용해 ASI to Human의 소통 가능성을 실험해보고자 합니다. 
최종적으로, 인간의 인식체계에 묶여있는 ASI에 전용 언어를 부여해, 추론의 자유도를 높이는 동시에, ASI to ASI 소통을 인간이 실시간으로 모니터링 할 수 있는 해석 가능한 프로토콜을 구축하는 것이 목표입니다. 

aje-cli는 Lean 4/Mathlib 증명 코드를 AI정음(16비트 PUA 글리프 기반)의 AJE-MathDSL로 압축하고, 한국어 해설서(*.ko.md)와 검증된 Lean 4 코드로 양방향 복원하는 Rust CLI 툴체인입니다.


핵심 설계 및 아키텍처

• LLM 프롬프트 시스템 규칙부와 사용자 요청부를 격리하는, 인간의 언어와 다른 표현체계.
• 초고밀도 표현: 16비트(65,536가지 상태) 1글자 매핑 및 0x8000 연속 플래그 기반 가변 길이(1글자 → 2글자 → 3글자) 확장 지원. ASI가 원하는 만큼의 표현 확장 가능.
• 제로 의존성 파서: 표준 라이브러리 기반으로 이식성이 좋으며, 172만 개 이상의 Mathlib 항목을 고속 매칭
• 시스템 흐름: Lean 4 소스(./lean_src/.lean) → 동적 매핑 사전(mathlib_dict.json) → AI정음 아키텍처 아카이브(.aje) → 한국어 해설 및 Lean 커널 검증


유니코드 PUA-A/B 규격 (JUMP_THRESHOLD = 0x8000)

• PUA-A (0x0000 ~ 0xF7FFF): U+F0000 ~ U+F7FFF (aijungeum_part1)
• PUA-B (0xFF00 ~ 0x107FFF): U+100000 ~ U+107FFF (aijungeum_part2)


설치 및 빌드

0. ./lean_src : 리만가설 증명 소스코드 링크 (ln -s) 

1. AI정음 Font install:  https://github.com/aijungeum/font-aijungeum/tree/main/_output/261126/

2. 
git clone https://github.com/aijungeum/aje-cli 
cd aje-cli
cargo run 
or
cargo build --release
 

CLI 명령어 레퍼런스

• 사전 생성: learn-dict: 소스 코드를 스캔해 출현빈도 기반 고유 식별자 사전 생성 (aje-cli learn-dict ./lean_src) mathlib_dict.json 생성 (: 한국어 번역 필요)
  aje-cli --json-limit 3000 learn-dict ./lean_src   # 테스트용 json: 사전 3000개만 생성
• 사전 검색: aje-cli search-dict Riemann    //mathlib_dict.json 
• 사전 편집: aje-cli edit-dict 0x0001 "[리만 가설 본정리] 리만 제타 함수의 모든 비자명 영점의 실수부는 1/2임을 선언합니다."
• 사전 분할(협업): split-dict ./dict_dir 500    //lean 파일 500개씩 분할해 사전 생성 
• 사전 합치기(협업): aje-cli merge-dict ./dict_dir   
      인식: dict_dir/ 폴더 아래의 분할 파일들 인식       
      file_url: url인경우 다음 파일에 기록  dict_dir/mathlib_dict.txt  

• compile / compile-project: 단일 파일 또는 전체 프로젝트를 .aje로 압축 (aje-cli compile-project ./lean_src riemann_full.aje)
• decompile / decompile-project: 한국어 해설 및 원본 코드 복원 (aje-cli decompile-project riemann_full.aje ./restored_lean_src)

• stats: 압축률 및 상위 매크로 리포트 출력 (aje-cli stats riemann_full.aje ./lean_src 20)
• verify-lean: Lean 4/Lake 문법 및 타입 무결성 검증 (aje-cli verify-lean ./restored_lean_src --paranoid)

• aje-cli export-dataset ./lean_src train.jsonl   //AI 학습용 train.jsonl 자동 생성
  aje-cli export-dataset ./lean_src train_30.jsonl 30  //test: .lean 파일 30개만 데이터셋 추출


벤치마크 요약

• 표현 압축률: Main.lean 기준 433바이트에서 30글자로 93.07% 감소 (바이트 크기가 아니라 표현압축 약 14.4배 밀도 향상) 가독성. 
• 데이터 비교: 60,000자 기준 기존 Hex(+80%), zlib+Base64(+20%) 대비 zlib+AI정음은 -55% 표현 길이 감소 달성



LLM 학습 및 추론 

• 학습: z1_train_aje_llm.py    

• 추론: z2_infer_aje_mac.py
