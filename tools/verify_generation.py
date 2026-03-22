#!/usr/bin/env python3
"""
verify_generation.py — Quick check: run full ChatML generation using float32
HuggingFace model to see baseline output, then verify our BPE tokenization
assembles the same prompt.
"""

import sys
from transformers import AutoModelForCausalLM, AutoTokenizer
import torch

model_name = "HuggingFaceTB/SmolLM2-135M-Instruct"
print(f"Loading {model_name}...")
model = AutoModelForCausalLM.from_pretrained(model_name, dtype=torch.float32)
tokenizer = AutoTokenizer.from_pretrained(model_name)
model.eval()

# Our system prompt (LOCAL_SYSTEM_PROMPT from kernel code)
system_prompt = "You are hackbot, a kernel agent. Answer concisely. For live system data, use: <tool>ps</tool> <tool>mem</tool> <tool>loadavg</tool>"

# Build ChatML
messages = [
    {"role": "system", "content": system_prompt},
    {"role": "user", "content": "hello"},
]

# Method 1: Using apply_chat_template
chat_text = tokenizer.apply_chat_template(messages, tokenize=False, add_generation_prompt=True)
print(f"\nChatML text ({len(chat_text)} chars):")
print(repr(chat_text))

tokens = tokenizer.encode(chat_text, add_special_tokens=False)
print(f"\nTokens ({len(tokens)}): {tokens}")

# Generate with greedy decoding (matching kernel argmax)
print(f"\n=== Generation (greedy, max_new_tokens=128) ===")
input_ids = torch.tensor([tokens])
with torch.no_grad():
    output = model.generate(
        input_ids,
        max_new_tokens=128,
        do_sample=False,
        temperature=1.0,
    )

generated_ids = output[0][len(tokens):].tolist()
generated_text = tokenizer.decode(generated_ids, skip_special_tokens=False)
print(f"Generated token IDs: {generated_ids}")
print(f"Generated text: {repr(generated_text)}")

# Print token-by-token
print(f"\nToken-by-token:")
for i, tid in enumerate(generated_ids[:30]):
    tok_str = tokenizer.decode([tid])
    print(f"  gen[{i}]: token {tid} = {repr(tok_str)}")

# Also check: what does the model predict after the full prompt?
print(f"\n=== Logits check after full prompt ===")
with torch.no_grad():
    logits = model(input_ids).logits[0, -1]  # last position

top5 = torch.topk(logits, 5)
print(f"Top-5 after full prompt ({len(tokens)} tokens):")
for i in range(5):
    tid = top5.indices[i].item()
    val = top5.values[i].item()
    tok_str = tokenizer.decode([tid])
    print(f"  {i+1}. token {tid} = {repr(tok_str)} (logit={val:.4f})")

# Also test with the echo prompt specifically
# When user does: echo "hello" > /dev/hackbot
# The shell adds a newline, so prompt is "hello\n"
print(f"\n=== With trailing newline (echo behavior) ===")
messages2 = [
    {"role": "system", "content": system_prompt},
    {"role": "user", "content": "hello\n"},  # echo appends newline
]
chat_text2 = tokenizer.apply_chat_template(messages2, tokenize=False, add_generation_prompt=True)
tokens2 = tokenizer.encode(chat_text2, add_special_tokens=False)
print(f"Tokens with \\n ({len(tokens2)}): {tokens2}")

input_ids2 = torch.tensor([tokens2])
with torch.no_grad():
    output2 = model.generate(input_ids2, max_new_tokens=128, do_sample=False)

generated2 = tokenizer.decode(output2[0][len(tokens2):], skip_special_tokens=False)
print(f"Generated: {repr(generated2)}")
