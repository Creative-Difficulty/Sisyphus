"""Prompt classifier experiment: static multilingual embeddings, shrunk to
<= 20 MB, with an independent (one-vs-rest sigmoid) head per category.

Trains on data/train.jsonl (en, de), tests on data/test.jsonl (12 languages).
Prints per-variant size, accuracy per language, macro F1 and calibration.
"""

import gzip
import io
import json
import sys
import time
from pathlib import Path

import numpy as np
from sklearn.decomposition import PCA
from sklearn.feature_extraction.text import TfidfVectorizer
from sklearn.linear_model import LogisticRegression
from sklearn.metrics import f1_score
from tokenizers import Tokenizer

ROOT = Path(__file__).parent
ACTS = ["new_request", "correction", "clarification", "approval", "question",
        "stop_undo", "handoff", "chit_chat", "other"]
TASKS = ["feature", "bug_fix", "error_paste", "refactor", "understand_code",
         "learn_concept", "ui_styling", "tests", "setup_deploy", "docs_writing",
         "planning_design", "review", "other"]
CATS = [f"act:{a}" for a in ACTS] + [f"task:{t}" for t in TASKS]


def load(name):
    rows = [json.loads(l) for l in open(ROOT / "data" / name) if l.strip()]
    y = np.zeros((len(rows), len(CATS)), dtype=int)
    for i, r in enumerate(rows):
        y[i, CATS.index(f"act:{r['act']}")] = 1
        y[i, CATS.index(f"task:{r['task']}")] = 1
    return rows, y


# ---------------------------------------------------------------------------
# Embedders


class Static:
    """Tokenize, look up rows, mean, L2-normalize (what model2vec does)."""

    def __init__(self, tokenizer, emb, scale=None, size_bytes=None, name=""):
        self.tok, self.emb, self.scale = tokenizer, emb, scale
        self.size_bytes, self.name = size_bytes, name

    def encode(self, texts):
        out = np.zeros((len(texts), self.emb.shape[1]), dtype=np.float32)
        for i, enc in enumerate(self.tok.encode_batch(texts, add_special_tokens=False)):
            ids = enc.ids
            if not ids:
                continue
            v = self.emb[ids].astype(np.float32).mean(0)
            if self.scale is not None:
                v *= self.scale
            n = np.linalg.norm(v)
            out[i] = v / n if n > 0 else v
        return out


def full_model(dirname):
    from safetensors.numpy import load_file
    d = ROOT / "models" / dirname
    emb = load_file(d / "model.safetensors")["embeddings"]
    tok = Tokenizer.from_file(str(d / "tokenizer.json"))
    return Static(tok, emb, size_bytes=emb.nbytes, name=f"{dirname} (full)")


def shrunk_unigram(dirname, keep, dims, bits=8):
    """Keep the `keep` most likely Unigram pieces (plus specials), PCA to
    `dims`, quantize to int8 per dimension."""
    from safetensors.numpy import load_file
    d = ROOT / "models" / dirname
    emb = load_file(d / "model.safetensors")["embeddings"]
    spec = json.load(open(d / "tokenizer.json"))
    vocab = spec["model"]["vocab"]  # [[piece, score], ...]
    specials = {t["id"] for t in spec.get("added_tokens", [])}
    unk = spec["model"].get("unk_id")
    order = sorted((i for i in range(len(vocab)) if i not in specials),
                   key=lambda i: -vocab[i][1])
    kept = sorted(specials | ({unk} if unk is not None else set()) | set(order[:keep]))
    remap = {old: new for new, old in enumerate(kept)}
    spec["model"]["vocab"] = [vocab[i] for i in kept]
    if unk is not None:
        spec["model"]["unk_id"] = remap[unk]
    for t in spec.get("added_tokens", []):
        t["id"] = remap[t["id"]]
    tok_json = json.dumps(spec, ensure_ascii=False)
    tok = Tokenizer.from_str(tok_json)
    e = emb[kept]
    if dims < e.shape[1]:
        e = PCA(n_components=dims, random_state=0).fit_transform(e).astype(np.float32)
    scale = None
    if bits == 8:
        scale = np.abs(e).max(0) / 127.0
        scale[scale == 0] = 1.0
        e = np.round(e / scale).astype(np.int8)
    # Size: the embedding table plus the tokenizer, both gzip-compressed as
    # they would ship.
    table = gzip.compress(e.tobytes(), 6)
    tokz = gzip.compress(tok_json.encode(), 9)
    size = len(table) + len(tokz) + (dims * 4)
    return Static(tok, e, scale=scale, size_bytes=size,
                  name=f"{dirname} top {keep // 1000}k x {dims}d int{bits}")


class CharNgrams:
    """Baseline: our own tiny model, no pretrained knowledge."""

    def __init__(self):
        self.vec = TfidfVectorizer(analyzer="char_wb", ngram_range=(2, 4),
                                   max_features=50000, sublinear_tf=True)
        self.name, self.size_bytes = "char n-grams, own (no pretrained)", None

    def fit(self, texts):
        self.vec.fit(texts)
        self.size_bytes = len(self.vec.vocabulary_) * (6 + 4 * len(CATS))

    def encode(self, texts):
        return self.vec.transform(texts)


# ---------------------------------------------------------------------------
# Head and evaluation


def train_head(X, y, C):
    heads = []
    for j in range(y.shape[1]):
        clf = LogisticRegression(C=C, max_iter=2000, class_weight="balanced")
        clf.fit(X, y[:, j])
        heads.append(clf)
    return heads


def predict(heads, X):
    return np.stack([h.predict_proba(X)[:, 1] for h in heads], axis=1)


def ece(p, y, bins=10):
    """Expected calibration error over all category probabilities."""
    p, y = p.ravel(), y.ravel()
    edges = np.linspace(0, 1, bins + 1)
    total = 0.0
    for lo, hi in zip(edges[:-1], edges[1:]):
        m = (p >= lo) & (p < hi if hi < 1 else p <= hi)
        if m.any():
            total += m.mean() * abs(p[m].mean() - y[m].mean())
    return total


def fit_temperature(p_val, y_val):
    """One temperature on the logits, fitted on held-out training data."""
    logit = np.log(np.clip(p_val, 1e-6, 1 - 1e-6) / np.clip(1 - p_val, 1e-6, 1))
    best, best_t = 1e9, 1.0
    for t in np.linspace(0.3, 5, 95):
        q = 1 / (1 + np.exp(-logit / t))
        loss = -(y_val * np.log(q + 1e-9) + (1 - y_val) * np.log(1 - q + 1e-9)).mean()
        if loss < best:
            best, best_t = loss, t
    return best_t


def apply_temperature(p, t):
    logit = np.log(np.clip(p, 1e-6, 1 - 1e-6) / np.clip(1 - p, 1e-6, 1))
    return 1 / (1 + np.exp(-logit / t))


def evaluate(name, size, p, y, rows):
    na = len(ACTS)
    act_ok = p[:, :na].argmax(1) == y[:, :na].argmax(1)
    task_ok = p[:, na:].argmax(1) == y[:, na:].argmax(1)
    f1 = f1_score(y, (p >= 0.5).astype(int), average="macro", zero_division=0)
    langs = sorted({r["lang"] for r in rows}, key=lambda l: (l not in ("en", "de"), l))
    per = {}
    for l in langs:
        m = np.array([r["lang"] == l for r in rows])
        per[l] = (act_ok[m].mean(), task_ok[m].mean())
    return {
        "name": name, "size_mb": None if size is None else size / 1e6,
        "act": act_ok.mean(), "task": task_ok.mean(), "f1": f1, "ece": ece(p, y),
        "per_lang": per,
    }


def run(embedder, train_rows, ytr, test_rows, yte):
    texts_tr = [r["text"] for r in train_rows]
    texts_te = [r["text"] for r in test_rows]
    if hasattr(embedder, "fit"):
        embedder.fit(texts_tr)
    t0 = time.perf_counter()
    Xtr = embedder.encode(texts_tr)
    Xte = embedder.encode(texts_te)
    ms = (time.perf_counter() - t0) * 1000 / (len(texts_tr) + len(texts_te))
    # Pick C and the temperature on a held-out fifth of the training data.
    rng = np.random.default_rng(0)
    idx = rng.permutation(len(texts_tr))
    val, fit = idx[: len(idx) // 5], idx[len(idx) // 5:]
    best = None
    for C in (0.3, 1, 3, 10, 30):
        h = train_head(Xtr[fit], ytr[fit], C)
        pv = predict(h, Xtr[val])
        score = (pv[:, :len(ACTS)].argmax(1) == ytr[val, :len(ACTS)].argmax(1)).mean() + \
                (pv[:, len(ACTS):].argmax(1) == ytr[val, len(ACTS):].argmax(1)).mean()
        if best is None or score > best[0]:
            best = (score, C, pv)
    _, C, pv = best
    T = fit_temperature(pv, ytr[val])
    heads = train_head(Xtr, ytr, C)
    p = apply_temperature(predict(heads, Xte), T)
    res = evaluate(embedder.name, embedder.size_bytes, p, yte, test_rows)
    res.update(C=C, T=T, ms=ms)
    return res, p


def main():
    train_rows, ytr = load("train.jsonl")
    test_rows, yte = load("test.jsonl")
    print(f"train {len(train_rows)}, test {len(test_rows)}", file=sys.stderr)
    variants = [CharNgrams]
    for base in ["potion-multilingual-128M", "M2V_multilingual_output", "blink-multilingual-64M"]:
        variants.append(lambda b=base: full_model(b))
    for keep, dims in [(250_000, 64), (150_000, 96), (100_000, 128),
                       (60_000, 192), (100_000, 64), (50_000, 128)]:
        variants.append(lambda k=keep, d=dims: shrunk_unigram("potion-multilingual-128M", k, d))
    results = []
    for make in variants:
        emb = make()
        print(f"running {emb.name}", file=sys.stderr)
        res, p = run(emb, train_rows, ytr, test_rows, yte)
        results.append(res)
        print(json.dumps(res, default=float), flush=True)
    json.dump(results, open(ROOT / "results.json", "w"), default=float, indent=1)


if __name__ == "__main__":
    main()
