"""Train the final prompt model and compare the shrunk variants.

Privacy: fine-tuning may only change embedding rows of word pieces that occur
in the public training data (written by LLMs, no personal data). The real
(private) prompts still train the head and the shared rows, but no word piece
that only occurs in them is changed, so the published table doesn't reveal
their vocabulary.

usage: final.py   -> trains, saves final-model.pkl, prints variant scores
"""

import glob
from pathlib import Path
import json
import pickle
import random

import numpy as np
import torch
from tokenizers import Tokenizer

import experiment as ex
import round4
import round6
import typos

PUBLIC = ["data/train.jsonl"] + sorted(glob.glob("data/translate/??.jsonl")) + sorted(glob.glob("data/gen/*.jsonl"))
# Real prompts, labelled the same way, weighted 3x. Not in the repository
# (they're private); training works without them, with somewhat lower
# accuracy on real prompts.
PRIVATE = ["data/real/train-ab.jsonl"] * 3 if Path("data/real/train-ab.jsonl").exists() else []


def rows_of(files):
    rows, _ = round4.load_rows(files)
    return [{k: r[k] for k in ("text", "lang", "act", "task")} for r in rows]


def main():
    public = rows_of(PUBLIC)
    private = rows_of(PRIVATE) if PRIVATE else []
    public_all = public + typos.augment(public, copies=3, seed=5)
    private_all = private + typos.augment(private, copies=3, seed=6)
    tok = Tokenizer.from_file(str(round4.ROOT / "models" / round4.BASE / "tokenizer.json"))
    allowed = set()
    for enc in tok.encode_batch([r["text"] for r in public_all], add_special_tokens=False):
        allowed.update(enc.ids)
    print(f"public {len(public_all)} rows, private {len(private_all)} rows; {len(allowed)} word pieces may change")

    from model2vec.train import base
    mask = torch.zeros(250000, 1)
    mask[sorted(allowed)] = 1.0
    original = base.BaseFinetuneable.construct_embeddings

    def masked(self):
        emb = original(self)
        emb.weight.register_hook(lambda g: g * mask.to(g.device))
        return emb

    base.BaseFinetuneable.construct_embeddings = masked
    rows = public_all + private_all
    random.Random(0).shuffle(rows)
    _, model = round4.fine_tuned_table(rows, n_layers=2, hidden_dim=512)
    base.BaseFinetuneable.construct_embeddings = original

    tuned = model.embeddings.weight.detach().cpu().numpy()
    orig = model.original.cpu().numpy()
    changed = set(np.where(np.abs(tuned - orig).max(1) > 1e-6)[0].tolist())
    leaked = changed - allowed
    print(f"changed rows {len(changed)}, outside public vocabulary: {len(leaked)}")
    assert not leaked
    pickle.dump(model, open("final-model.pkl", "wb"))

    for path in ["test.jsonl", "test-typos.jsonl", "real/test-agreed.jsonl"]:
        if not Path("data", path).exists():
            continue
        round6.evaluate(model, "final (uncompressed)", None, path)


if __name__ == "__main__":
    main()
