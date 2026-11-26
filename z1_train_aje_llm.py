import os
import json
import gc
import torch
from datasets import load_dataset
from transformers import (
    AutoTokenizer,
    AutoModelForCausalLM,
    TrainingArguments,
    Trainer,
    DataCollatorForSeq2Seq,
)
from peft import LoraConfig, get_peft_model

# 맥북 MPS 메모리 상한 해제 방지 및 파편화 최소화
os.environ["PYTORCH_MPS_HIGH_WATERMARK_RATIO"] = "0.0"

#
BASE_MODEL = "Qwen/Qwen2.5-0.5B-Instruct"
# BASE_MODEL = "Qwen/Qwen2.5-1.5B-Instruct"
# BASE_MODEL = "Qwen/Qwen2.5-3B-Instruct"
DATASET_PATH = "train.jsonl"
OUTPUT_DIR = "./aje-math-llm-lora-mac"

PUA_A_START = 0xF0000
PUA_A_LIMIT = 0xF7FFF  # 32,768개 (0xF0000 ~ 0xF7FFF)
PUA_B_START = 0x100000
PUA_B_LIMIT = 0x107FFF  # 32,768개 (0x100000 ~ 0x107FFF)
JUMP_THRESHOLD = 0x8000


def get_mac_device() -> str:
    return "mps" if torch.backends.mps.is_available() else "cpu"


def extract_used_aje_chars(jsonl_path: str) -> list[str]:
    aje_chars = set()
    with open(jsonl_path, "r", encoding="utf-8") as f:
        for line in f:
            if not line.strip():
                continue
            record = json.loads(line)
            for field in ("input", "output"):
                for ch in record.get(field, ""):
                    cp = ord(ch)
                    if (PUA_A_START <= cp <= PUA_A_LIMIT) or (PUA_B_START <= cp <= PUA_B_LIMIT):
                        aje_chars.add(ch)
    return sorted(aje_chars, key=ord)


def main():
    device = get_mac_device()
    print(f"0. 실행 디바이스: {device.upper()} (메모리 최적화 모드)")

    new_aje_tokens = extract_used_aje_chars(DATASET_PATH)
    print(f"1. 추출된 AI정음 토큰 수: {len(new_aje_tokens)}개")

    tokenizer = AutoTokenizer.from_pretrained(BASE_MODEL, trust_remote_code=True)
    if tokenizer.pad_token is None:
        tokenizer.pad_token = tokenizer.eos_token

    tokenizer.add_tokens(new_aje_tokens)

    dtype = torch.float16 if device == "mps" else torch.float32
    model = AutoModelForCausalLM.from_pretrained(
        BASE_MODEL,
        torch_dtype=dtype,
        low_cpu_mem_usage=True,
        trust_remote_code=True,
    )

    model.resize_token_embeddings(len(tokenizer))

    # 핵심 메모리 절약 1: Gradient Checkpointing 활성화
    model.gradient_checkpointing_enable()
    model.enable_input_require_grads()
    model.to(device)

    # 핵심 메모리 절약 2: 거대한 embed_tokens/lm_head 복사(modules_to_save) 제거
    lora_config = LoraConfig(
        r=8,
        lora_alpha=16,
        target_modules=["q_proj", "v_proj"],
        lora_dropout=0.05,
        bias="none",
        task_type="CAUSAL_LM",
    )
    model = get_peft_model(model, lora_config)
    model.print_trainable_parameters()

    raw_dataset = load_dataset("json", data_files=DATASET_PATH, split="train")

    def tokenize_sample(example):
        messages = [
            {"role": "system", "content": "Lean 4와 AI정음(AJE)을 통역하는 수학 AI입니다."},
            {"role": "user", "content": f"{example['instruction']}\n\n{example['input']}"},
            {"role": "assistant", "content": example["output"]},
        ]
        full_text = tokenizer.apply_chat_template(messages, tokenize=False, add_generation_prompt=False)
        # 핵심 메모리 절약 3: 최대 토큰 길이를 512로 제한해 어텐션 메모리 폭발 방지
        tokenized = tokenizer(full_text, truncation=True, max_length=512)
        tokenized["labels"] = tokenized["input_ids"].copy()
        return tokenized

    tokenized_dataset = raw_dataset.map(tokenize_sample, remove_columns=raw_dataset.column_names)

    gc.collect()
    if device == "mps":
        torch.mps.empty_cache()

    training_args = TrainingArguments(
        output_dir=OUTPUT_DIR,
        num_train_epochs=3,
        per_device_train_batch_size=1,
        gradient_accumulation_steps=8,
        learning_rate=2e-4,
        fp16=False,
        logging_steps=5,
        save_strategy="epoch",
        optim="adafactor",  # AdamW 대신 메모리 점유율이 절반 이하인 Adafactor 옵티마이저 사용
        dataloader_pin_memory=False,
        report_to="none",
    )

    trainer = Trainer(
        model=model,
        args=training_args,
        train_dataset=tokenized_dataset,
        data_collator=DataCollatorForSeq2Seq(tokenizer=tokenizer, padding=True),
    )

    print("2. 저메모리 파인튜닝 시작...")
    trainer.train()

    model.save_pretrained(OUTPUT_DIR)
    tokenizer.save_pretrained(OUTPUT_DIR)
    print(f"3. 완료! 모델 저장 위치: '{OUTPUT_DIR}'")


if __name__ == "__main__":
    main()
