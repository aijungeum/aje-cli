# python3 infer_aje_mac.py main.aje
import sys
import os
import torch
from transformers import AutoTokenizer, AutoModelForCausalLM
from peft import PeftModel

# 학습 스크립트(train_aje_llm.py)와 동일한 설정 경로
BASE_MODEL = "Qwen/Qwen2.5-0.5B-Instruct"
# BASE_MODEL = "Qwen/Qwen2.5-1.5B-Instruct"
# BASE_MODEL = "Qwen/Qwen2.5-3B-Instruct"
LORA_DIR = "./aje-math-llm-lora-mac"


def get_mac_device() -> str:
    if torch.backends.mps.is_available():
        return "mps"
    return "cpu"


def load_trained_aje_model():
    device = get_mac_device()
    print(f"1. 실행 디바이스: {device.upper()} (Apple Silicon Metal 가속)")

    if not os.path.exists(LORA_DIR):
        raise FileNotFoundError(
            f"학습된 모델 폴더('{LORA_DIR}')를 찾을 수 없습니다. 먼저 `python3 train_aje_llm.py`를 실행하세요."
        )

    print(f"2. 확장된 AI정음 토크나이저 로드 중 ({LORA_DIR})...")
    tokenizer = AutoTokenizer.from_pretrained(LORA_DIR, trust_remote_code=True)
    if tokenizer.pad_token is None:
        tokenizer.pad_token = tokenizer.eos_token

    print(f"3. 베이스 모델({BASE_MODEL}) 로드 및 LoRA 가중치 병합 중...")
    dtype = torch.float16 if device == "mps" else torch.float32
    base_model = AutoModelForCausalLM.from_pretrained(
        BASE_MODEL,
        torch_dtype=dtype,
        trust_remote_code=True,
    )

    # 중요: 학습 시 확장된 AI정음 토큰 수만큼 임베딩 크기를 먼저 맞춘 뒤 LoRA 로드
    base_model.resize_token_embeddings(len(tokenizer))

    model = PeftModel.from_pretrained(base_model, LORA_DIR)
    model.to(device)
    model.eval()
    return model, tokenizer, device


def generate_response(model, tokenizer, device: str, instruction: str, aje_input: str, max_new_tokens: int = 512) -> str:
    messages = [
        {
            "role": "system",
            "content": "당신은 Lean 4 형식 증명과 AI정음(AiJungEum) 고밀도 프로토콜을 양방향으로 통역하는 수학 AI입니다.",
        },
        {"role": "user", "content": f"{instruction}\n\n{aje_input}"},
    ]

    prompt_text = tokenizer.apply_chat_template(messages, tokenize=False, add_generation_prompt=True)
    inputs = tokenizer(prompt_text, return_tensors="pt").to(device)

    with torch.no_grad():
        outputs = model.generate(
            **inputs,
            max_new_tokens=max_new_tokens,
            do_sample=False,          # 수학 증명의 정확성을 위해 결정론적(Greedy) 디코딩 사용
            repetition_penalty=1.05,
            pad_token_id=tokenizer.pad_token_id,
            eos_token_id=tokenizer.eos_token_id,
        )

    # 입력 프롬프트 길이를 제외한 순수 생성 토큰만 디코딩
    generated_ids = outputs[0][inputs["input_ids"].shape[1]:]
    return tokenizer.decode(generated_ids, skip_special_tokens=True).strip()


def main():
    # 커맨드라인 인자로 .aje 파일 경로를 받음 (기본값: main.aje)
    aje_file_path = sys.argv[1] if len(sys.argv) > 1 else "main.aje"

    if not os.path.exists(aje_file_path):
        print(f"[오류] 입력 파일('{aje_file_path}')이 없습니다.")
        print("사용법: python3 infer_aje_mac.py <입력_파일.aje>")
        sys.exit(1)

    with open(aje_file_path, "r", encoding="utf-8") as f:
        aje_stream = f.read().strip()

    print(f"▶ 입력된 AI정음 파일: '{aje_file_path}' (총 {len(aje_stream)} 글자)")
    model, tokenizer, device = load_trained_aje_model()

    print("\n==================================================================")
    print("  [추론 1] AI정음 -> 비전공자용 한국어 직관 해설 생성 중...")
    print("==================================================================")
    ko_result = generate_response(
        model,
        tokenizer,
        device,
        "주어진 AI정음(AJE-MathDSL) 증명 토큰 시퀀스를 분석하여 비전공자도 이해할 수 있는 단계별 한국어 논리 해설로 설명하시오.",
        aje_stream,
    )
    print(ko_result)

    print("\n==================================================================")
    print("  [추론 2] AI정음 -> 실행 가능한 Lean 4 형식 코드 복원 중...")
    print("==================================================================")
    lean_result = generate_response(
        model,
        tokenizer,
        device,
        "주어진 AI정음(AJE-MathDSL) 토큰 시퀀스를 해석하여 실행 가능한 Lean 4 형식 증명 코드로 복원하시오.",
        aje_stream,
    )
    print(lean_result)
    print("==================================================================")


if __name__ == "__main__":
    main()
