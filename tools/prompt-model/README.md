# Prompt model

Trains `models/sisyphus-prompt-1.bin`, the local classifier that gives every
prompt a probability for each of 22 categories (`labels.md`: 9 kinds of
message, 13 topics). The app runs it in `crates/client/src/prompt_model/`.

## How it works

- Base: [minishlab/blink-multilingual-64M](https://huggingface.co/minishlab/blink-multilingual-64M)
  (MIT), a static embedding table (250,000 word pieces x 256) for 100+
  languages. A prompt is the average of its word pieces' rows.
- The table and a small classifier (256 -> 512 -> 512 -> 22, sigmoid per
  category) are fine-tuned together with model2vec.
- Privacy: fine-tuning may only change rows of word pieces that occur in the
  public (LLM-written) training data. The private real prompts train the
  classifier and those shared rows, so the published file doesn't reveal
  their vocabulary.
- Shipped size: the original table as 64 one-byte product-quantization codes
  per row, the ~29,000 fine-tuned rows as int8, the classifier as int8,
  the tokenizer: 26.7 MB.

## Data

- `data/train.jsonl`: 1,069 English and German prompts; `data/translate/`:
  250 of them adapted into each of 24 more languages; `data/gen/`: 1,200
  prompts for the weak spots. All written by LLMs.
- Every prompt also gets three copies with typos (`typos.py`).
- `data/test.jsonl`: 720 prompts in 12 languages; Polish, Turkish, Japanese
  and Arabic are never in training. `data/test-typos.jsonl`: the same with
  typos.
- The published model was also trained on 401 real prompts (weighted 3x),
  which aren't in this repository.

Results of the published model (top act / top task right):

| Test | Act | Task |
|---|---|---|
| Synthetic, 12 languages | 68% | 69% |
| Synthetic with typos | 58% | 59% |
| 174 real prompts (one person, mostly English) | 71% | 55% |

## Rebuild

```sh
uv venv --python 3.12 .venv
uv pip install --python .venv/bin/python numpy scikit-learn "model2vec[train]" tokenizers safetensors huggingface_hub
uv pip install --python .venv/bin/python torch --index-url https://download.pytorch.org/whl/cpu
.venv/bin/python -c "from huggingface_hub import snapshot_download; snapshot_download('minishlab/blink-multilingual-64M', local_dir='models/blink-multilingual-64M')"
.venv/bin/python final.py                  # fine-tune, saves final-model.pkl
.venv/bin/python export.py compare         # score the int8 and pq variants
.venv/bin/python export.py write int8 sisyphus-prompt-1.bin
```

A new model needs a new name (`sisyphus-prompt-2`), its SHA-256 and size in
`crates/client/src/prompt_model/mod.rs`, a new parity file
(`parity.py sisyphus-prompt-2.bin ../../crates/client/tests/data/prompt_model_parity.json`)
and the server route.

The other scripts are the experiments that led here: `experiment.py`
(helpers, first round), `round4.py` (fine-tuning and compression),
`round6.py` (evaluation), `typos.py`.
