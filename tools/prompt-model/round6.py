"""Round 6: real prompts and gap-filling prompts.

usage: round6.py TAG train-file [...]
Trains the 25 MB setup (fine-tuned blink, PQ64 base + int8 tuned rows,
2-layer head) on the given files plus 3 typo copies of them, then evaluates
on the synthetic test, the synthetic typo test and the real test (agreed
labels only).
"""

import json
import pickle
import random
import sys

import numpy as np

import experiment as ex
import round4
import typos


def evaluate(model, name, size, path):
    rows, y = ex.load(path)
    p = np.asarray(model.predict_proba([r["text"] for r in rows]))
    classes = list(model.classes_)
    p = p[:, [classes.index(c) for c in ex.CATS]]
    res = ex.evaluate(name, size, p, y, rows)
    na = len(ex.ACTS)
    act_ok = p[:, :na].argmax(1) == y[:, :na].argmax(1)
    task_ok = p[:, na:].argmax(1) == y[:, na:].argmax(1)
    print(f"  {path:28s} act {act_ok.mean():.0%}  task {task_ok.mean():.0%}  "
          f"F1 {res['f1']:.2f}  ECE {res['ece']:.3f}  (n={len(rows)})", flush=True)
    return res, p


def main():
    tag, files = sys.argv[1], sys.argv[2:]
    rows, _ = round4.load_rows(files)
    aug = typos.augment(rows, copies=3, seed=5)
    tmp = f"data/_train-{tag}.jsonl"
    with open(tmp, "w") as f:
        for r in rows + aug:
            f.write(json.dumps({k: r[k] for k in ("text", "lang", "act", "task")}, ensure_ascii=False) + "\n")
    all_rows, _ = round4.load_rows([tmp])
    print(f"[{tag}] {len(rows)} prompts + {len(aug)} typo copies", flush=True)
    _, model = round4.fine_tuned_table(all_rows, n_layers=2, hidden_dim=512)
    rows_te, yte = ex.load("test.jsonl")
    out = []
    import io, contextlib
    with contextlib.redirect_stdout(io.StringIO()):
        round4.hybrid_eval(model, rows_te, yte, round4.tokenizer_size(round4.BASE), tag, out,
                           m=64, rows_pq=None, restore=False)
    size = out[0]["size_mb"]
    print(f"[{tag}] shipped size {size:.1f} MB", flush=True)
    for path in ["test.jsonl", "test-typos.jsonl", "real/test-agreed.jsonl"]:
        evaluate(model, tag, size * 1e6, path)
    pickle.dump(model, open(f"model-{tag}.pkl", "wb"))


if __name__ == "__main__":
    main()
