"""Round 4: more languages in training, fine-tuned static embeddings.

usage: round4.py TAG train-file [train-file ...] [--variants a,b,c]

Test languages pl, tr, ja, ar are never in training (held out); the rest of
the test languages may be. Prints accuracy for seen and held-out languages
separately.
"""

import gzip
import json
import sys
from pathlib import Path

import numpy as np
from safetensors.numpy import load_file
from sklearn.cluster import MiniBatchKMeans
from tokenizers import Tokenizer

import experiment as ex

ROOT = Path(__file__).parent
HELD_OUT = {"pl", "tr", "ja", "ar"}
BASE = "blink-multilingual-64M"


def load_rows(files):
    rows = []
    for f in files:
        rows += [json.loads(l) for l in open(f) if l.strip()]
    assert not any(r["lang"] in HELD_OUT for r in rows), "held-out language in training"
    y = np.zeros((len(rows), len(ex.CATS)), dtype=int)
    for i, r in enumerate(rows):
        y[i, ex.CATS.index(f"act:{r['act']}")] = 1
        y[i, ex.CATS.index(f"task:{r['task']}")] = 1
    return rows, y


def pq(emb, m, seed=0):
    n, dim = emb.shape
    sub = dim // m
    codes = np.zeros((n, m), dtype=np.uint8)
    books = np.zeros((m, 256, sub), dtype=np.float32)
    for j in range(m):
        part = np.ascontiguousarray(emb[:, j * sub:(j + 1) * sub])
        km = MiniBatchKMeans(256, batch_size=8192, n_init=1, random_state=seed,
                             max_iter=50).fit(part)
        books[j] = km.cluster_centers_
        codes[:, j] = km.predict(part)
    rebuilt = np.concatenate([books[j][codes[:, j]] for j in range(m)], axis=1)
    size = len(gzip.compress(codes.tobytes(), 6)) + books.nbytes
    return rebuilt, size


def tokenizer_size(dirname):
    return len(gzip.compress(open(ROOT / "models" / dirname / "tokenizer.json", "rb").read(), 9))


def base_table():
    return load_file(ROOT / "models" / BASE / "model.safetensors")["embeddings"].astype(np.float32)


def fine_tuned_table(rows, epochs_seed=0, pq_first=None, n_layers=1, hidden_dim=512):
    """Fine-tune the static embeddings on our labels (model2vec), return the
    tuned table (token weights folded in) and the end-to-end model."""
    from model2vec.train import StaticModelForClassification
    model = StaticModelForClassification.from_pretrained(path=str(ROOT / "models" / BASE),
                                                         n_layers=n_layers, hidden_dim=hidden_dim)
    X = [r["text"] for r in rows]
    y = [[f"act:{r['act']}", f"task:{r['task']}"] for r in rows]
    model.pq_size = None
    if pq_first:
        # Compress first, then fine-tune on exactly what will ship.
        import torch
        rebuilt, model.pq_size = pq(model.embeddings.weight.detach().cpu().numpy().astype(np.float32), pq_first)
        model.vectors = torch.from_numpy(rebuilt)
    model.original = model.vectors.detach().clone()
    model.original_w = None if model.w is None else model.w.detach().clone()
    model.fit(X, y, random_seed=epochs_seed, device="cpu", early_stopping_patience=5,
              token_dropout=0.1)
    static = model.to_static_model()
    return np.asarray(static.embedding, dtype=np.float32), model


def hybrid_eval(model, rows_te, yte, tok_bytes, tag, out, m=32, rows_pq=None, restore=True):
    """Shippable fine-tuned model: the original table compressed with PQ,
    the rows fine-tuning changed stored separately as int8, the token
    weights as int8, and the model's own head."""
    import torch
    orig = model.original.cpu().numpy().astype(np.float32)
    tuned = model.embeddings.weight.detach().cpu().numpy().astype(np.float32)
    changed = np.where(np.abs(tuned - orig).max(1) > 1e-6)[0]
    if model.pq_size is not None:
        rebuilt, pq_size = orig.copy(), model.pq_size
    else:
        rebuilt, pq_size = pq(orig, m)
    rows = tuned[changed]
    if rows_pq:
        # The tuned rows get their own PQ codebooks.
        rq, rsize = pq(rows, rows_pq, seed=1)
        rebuilt[changed] = rq
        size = pq_size + rsize + 4 * len(changed) + tok_bytes
    else:
        scale = np.abs(rows).max(0) / 127.0
        scale[scale == 0] = 1
        q = np.round(rows / scale).astype(np.int8)
        rebuilt[changed] = q.astype(np.float32) * scale
        size = pq_size + len(gzip.compress(q.tobytes(), 6)) + 4 * len(changed) + tok_bytes
    if model.w is not None:
        w = model.w.detach().cpu().numpy()
        ws = np.abs(w).max() / 127.0
        wq = np.round(w / ws)
        size += len(gzip.compress(wq.astype(np.int8).tobytes(), 6))
        with torch.no_grad():
            model.w.copy_(torch.from_numpy((wq * ws).astype(np.float32)))
    size += sum(p.numel() for p in model.head.parameters()) * 4
    with torch.no_grad():
        model.embeddings.weight.copy_(torch.from_numpy(rebuilt))
    p = np.asarray(model.predict_proba([r["text"] for r in rows_te]))
    classes = list(model.classes_)
    p = p[:, [classes.index(c) for c in ex.CATS]]
    kind = "compressed first" if model.pq_size is not None else "compressed after"
    if rows_pq:
        kind += f", tuned rows PQ{rows_pq}"
    res = ex.evaluate(f"[{tag}] blink fine-tuned hybrid {kind} (PQ{m} + {len(changed)} int8 rows), own head",
                      size, p, yte, rows_te)
    res["ms"] = 0
    out.append(res)
    np.save(ROOT / f"pred-{tag}-pq{m}-{rows_pq}.npy", p)
    print(summarize(res, rows_te), flush=True)
    rows_ty, y_ty = ex.load("test-typos.jsonl")
    pt = np.asarray(model.predict_proba([r["text"] for r in rows_ty]))[:, [classes.index(c) for c in ex.CATS]]
    rt = ex.evaluate(res["name"] + " ON TYPO TEST", size, pt, y_ty, rows_ty)
    rt["ms"] = 0
    out.append(rt)
    print(summarize(rt, rows_ty), flush=True)
    if not restore:
        return
    with torch.no_grad():
        model.embeddings.weight.copy_(torch.from_numpy(tuned))


def summarize(res, rows_te):
    seen = [l for l in res["per_lang"] if l not in HELD_OUT]
    held = [l for l in res["per_lang"] if l in HELD_OUT]
    avg = lambda ls, k: np.mean([res["per_lang"][l][k] for l in ls])
    return (f"{res['name']}: {'' if res['size_mb'] is None else f'{res[chr(115)+chr(105)+chr(122)+chr(101)+chr(95)+chr(109)+chr(98)]:.1f} MB, '}"
            f"all act {res['act']:.0%} task {res['task']:.0%} | seen act {avg(seen, 0):.0%} task {avg(seen, 1):.0%} "
            f"| held-out act {avg(held, 0):.0%} task {avg(held, 1):.0%} | F1 {res['f1']:.2f} ECE {res['ece']:.3f}\n    "
            + " ".join(f"{l} {a:.0%}/{t:.0%}" for l, (a, t) in res["per_lang"].items()))


def main():
    args = sys.argv[1:]
    tag = args.pop(0)
    variants = "base_pq,tuned_pq,tuned_full"
    if "--variants" in args:
        i = args.index("--variants")
        variants = args[i + 1]
        del args[i:i + 2]
    variants = variants.split(",")
    rows_tr, ytr = load_rows(args)
    rows_te, yte = ex.load("test.jsonl")
    print(f"[{tag}] train {len(rows_tr)} rows, {len({r['lang'] for r in rows_tr})} languages", flush=True)
    tok = Tokenizer.from_file(str(ROOT / "models" / BASE / "tokenizer.json"))
    tok_bytes = tokenizer_size(BASE)
    out = []

    def run(emb, name, size):
        e = ex.Static(tok, emb, size_bytes=size, name=name)
        res, _ = ex.run(e, rows_tr, ytr, rows_te, yte)
        out.append(res)
        print(summarize(res, rows_te), flush=True)

    if "base_pq" in variants:
        table, size = pq(base_table(), 32)
        run(table, f"[{tag}] blink PQ32 + LR", size + tok_bytes)
    for v in [v for v in variants if v.startswith("qat")]:
        m = int(v[3:])
        _, model = fine_tuned_table(rows_tr, pq_first=m)
        hybrid_eval(model, rows_te, yte, tok_bytes, f"{tag}", out, m=m)
    if "int8rows" in variants:
        _, model = fine_tuned_table(rows_tr)
        hybrid_eval(model, rows_te, yte, tok_bytes, tag, out, m=64, rows_pq=64)
        hybrid_eval(model, rows_te, yte, tok_bytes, tag, out, m=64, rows_pq=None)
    if "deep" in variants:
        _, model = fine_tuned_table(rows_tr, n_layers=2, hidden_dim=512)
        hybrid_eval(model, rows_te, yte, tok_bytes, tag + "-deep", out, m=64, rows_pq=None)
    if "hyb64" in variants:
        _, model = fine_tuned_table(rows_tr)
        hybrid_eval(model, rows_te, yte, tok_bytes, tag, out, m=64, rows_pq=64)
        if "hyb64only" not in variants:
            hybrid_eval(model, rows_te, yte, tok_bytes, tag, out, m=64, rows_pq=128)
    if any(v.startswith("tuned") or v == "hybrid" for v in variants):
        tuned, model = fine_tuned_table(rows_tr)
        if "tuned_full" in variants:
            # The fine-tuned model's own head (MLP), uncompressed.
            p = np.asarray(model.predict_proba([r["text"] for r in rows_te]))
            classes = list(model.classes_)
            p = p[:, [classes.index(c) for c in ex.CATS]]
            res = ex.evaluate(f"[{tag}] blink fine-tuned, own head, full", None, p, yte, rows_te)
            res["ms"] = 0
            out.append(res)
            print(summarize(res, rows_te), flush=True)
        if "hybrid" in variants:
            hybrid_eval(model, rows_te, yte, tok_bytes, tag, out)
        if "tuned_pq" in variants:
            table, size = pq(tuned, 32)
            run(table, f"[{tag}] blink fine-tuned PQ32 + LR", size + tok_bytes)
        if "tuned_pq64" in variants:
            table, size = pq(tuned, 64)
            run(table, f"[{tag}] blink fine-tuned PQ64 + LR", size + tok_bytes)
    json.dump(out, open(ROOT / f"results-round4-{tag}.json", "w"), default=float, indent=1)


if __name__ == "__main__":
    main()
