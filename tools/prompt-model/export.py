"""Export the final model to Sisyphus's prompt model format and check it.

usage: export.py compare           # score the shrunk variants with the numpy reference
       export.py write VARIANT OUT  # write the file (VARIANT: int8 or pq)

File format (little endian), version 1:
  b"SPMODEL1", u32 header length, header JSON, then sections, each a u32
  byte length followed by the bytes, in this order:
    vocab        WordPiece vocabulary, ids 0.., UTF-8, joined by "\\n"
    added        added tokens as "id\\tcontent" lines (matched first, longest)
    punctuation  u32 (first, last) code point ranges counted as punctuation
    other        u32 ranges of Unicode "other" (Cc Cf Cs Co Cn): removed
    books        f32 [subspaces][256][dim / subspaces]: PQ codebooks
    codes        u8 [vocab_size][subspaces]
    tuned_ids    u32 [tuned]: rows replaced by fine-tuned ones
    tuned        int8: f32 scale[dim] + i8 [tuned][dim]
                 pq:   f32 [subspaces][256][dim / subspaces] + u8 [tuned][subspaces]
    then per layer: f32 scale[out], i8 [out][in], f32 bias[out]
Inference: tokenize, drop [UNK], mean of rows, L2-normalize, layers with
ReLU between them, sigmoid. Outputs are in header["labels"] order.
"""

import hashlib
import json
import pickle
import struct
import sys
import unicodedata

import numpy as np
from tokenizers import Tokenizer

import experiment as ex
import round4

DIM, M = 256, 64
NAME = "sisyphus-prompt-1"


def ranges(pred):
    out, start = [], None
    for cp in range(0x110000):
        ok = pred(cp)
        if ok and start is None:
            start = cp
        elif not ok and start is not None:
            out.append((start, cp - 1))
            start = None
    if start is not None:
        out.append((start, 0x10FFFF))
    return out


def int8_rows(x):
    scale = np.abs(x).max(0) / 127.0
    scale[scale == 0] = 1.0
    return scale.astype(np.float32), np.round(x / scale).astype(np.int8)


def int8_layer(w):
    scale = np.abs(w).max(1) / 127.0
    scale[scale == 0] = 1.0
    return scale.astype(np.float32), np.round(w / scale[:, None]).astype(np.int8)


def build(model, variant):
    """The arrays that go into the file, and the decoded table and layers."""
    orig = model.original.cpu().numpy().astype(np.float32)
    tuned = model.embeddings.weight.detach().cpu().numpy().astype(np.float32)
    ids = np.where(np.abs(tuned - orig).max(1) > 1e-6)[0].astype(np.uint32)
    books, codes = pq_fit(orig, M, seed=0)
    table = decode(books, codes)
    if variant == "int8":
        scale, q = int8_rows(tuned[ids])
        tuned_part = (scale, q)
        table[ids] = q.astype(np.float32) * scale
    else:
        tb, tc = pq_fit(tuned[ids], M, seed=1)
        tuned_part = (tb, tc)
        table[ids] = decode(tb, tc)
    layers = []
    for mod in model.head:
        if hasattr(mod, "weight"):
            w = mod.weight.detach().cpu().numpy().astype(np.float32)
            b = mod.bias.detach().cpu().numpy().astype(np.float32)
            s, q = int8_layer(w)
            layers.append((s, q, b))
    # Output order: ours, not model2vec's sorted classes.
    order = [list(model.classes_).index(c) for c in ex.CATS]
    s, q, b = layers[-1]
    layers[-1] = (s[order], q[order], b[order])
    return dict(books=books, codes=codes, ids=ids, tuned=tuned_part, layers=layers, table=table)


def pq_fit(x, m, seed):
    from sklearn.cluster import MiniBatchKMeans
    n, dim = x.shape
    sub = dim // m
    books = np.zeros((m, 256, sub), dtype=np.float32)
    codes = np.zeros((n, m), dtype=np.uint8)
    for j in range(m):
        part = np.ascontiguousarray(x[:, j * sub:(j + 1) * sub])
        km = MiniBatchKMeans(256, batch_size=8192, n_init=1, random_state=seed, max_iter=50).fit(part)
        books[j] = km.cluster_centers_
        codes[:, j] = km.predict(part)
    return books, codes


def decode(books, codes):
    m = books.shape[0]
    return np.concatenate([books[j][codes[:, j]] for j in range(m)], axis=1).astype(np.float32)


def reference(parts, tok, texts):
    """What the Rust code must compute (token ids from the HF tokenizer)."""
    out = np.zeros((len(texts), len(ex.CATS)), dtype=np.float32)
    for i, enc in enumerate(tok.encode_batch(texts, add_special_tokens=False)):
        ids = [t for t in enc.ids if t != 1]
        v = parts["table"][ids].mean(0) if ids else np.zeros(DIM, dtype=np.float32)
        n = np.linalg.norm(v)
        x = v / max(n, 1e-12)
        for k, (s, q, b) in enumerate(parts["layers"]):
            x = (q.astype(np.float32) * s[:, None]) @ x + b
            if k < len(parts["layers"]) - 1:
                x = np.maximum(x, 0)
        out[i] = 1 / (1 + np.exp(-x))
    return out


def score(p, path):
    rows, y = ex.load(path)
    na = len(ex.ACTS)
    act = (p[:, :na].argmax(1) == y[:, :na].argmax(1)).mean()
    task = (p[:, na:].argmax(1) == y[:, na:].argmax(1)).mean()
    return f"act {act:.0%} task {task:.0%} ECE {ex.ece(p, y):.3f}"


def file_bytes(parts, tok_spec):
    vocab = tok_spec["model"]["vocab"]
    inv = sorted(vocab.items(), key=lambda kv: kv[1])
    assert [i for _, i in inv] == list(range(len(inv)))
    vocab_bytes = "\n".join(t for t, _ in inv).encode()
    added = "\n".join(f"{a['id']}\t{a['content']}" for a in tok_spec["added_tokens"]).encode()
    punct = ranges(lambda cp: unicodedata.category(chr(cp)).startswith("P"))
    other = ranges(lambda cp: unicodedata.category(chr(cp)) in ("Cc", "Cf", "Cs", "Co", "Cn"))
    pack = lambda rs: b"".join(struct.pack("<II", a, b) for a, b in rs)
    header = {
        "name": NAME, "labels": ex.CATS, "dim": DIM, "subspaces": M,
        "vocab_size": int(parts["codes"].shape[0]), "wordpiece_vocab": len(inv),
        "tuned_rows": int(len(parts["ids"])), "tuned_format": parts["variant"],
        "layers": [[int(q.shape[1]), int(q.shape[0])] for _, q, _ in parts["layers"]],
        "unk_id": vocab["[UNK]"], "max_input_chars_per_word": 100, "fixed_length": 100,
    }
    sections = [vocab_bytes, added, pack(punct), pack(other),
                parts["books"].astype("<f4").tobytes(), parts["codes"].tobytes(),
                parts["ids"].astype("<u4").tobytes()]
    a, b = parts["tuned"]
    sections.append(a.astype("<f4").tobytes() + b.tobytes())
    for s, q, bias in parts["layers"]:
        sections += [s.astype("<f4").tobytes(), q.tobytes(), bias.astype("<f4").tobytes()]
    h = json.dumps(header, separators=(",", ":")).encode()
    out = b"SPMODEL1" + struct.pack("<I", len(h)) + h
    for s in sections:
        out += struct.pack("<I", len(s)) + s
    return out


def main():
    model = pickle.load(open("final-model.pkl", "rb"))
    tok_path = round4.ROOT / "models" / round4.BASE / "tokenizer.json"
    tok = Tokenizer.from_file(str(tok_path))
    tok_spec = json.load(open(tok_path))
    tests = [t for t in ["test.jsonl", "test-typos.jsonl", "real/test-agreed.jsonl"]
             if (round4.ROOT / "data" / t).exists()]
    if sys.argv[1] == "compare":
        for variant in ("int8", "pq"):
            parts = build(model, variant)
            parts["variant"] = variant
            size = len(file_bytes(parts, tok_spec))
            for path in tests:
                rows, _ = ex.load(path)
                print(f"{variant:4s} {size / 1e6:5.1f} MB  {path:24s} " + score(reference(parts, tok, [r['text'] for r in rows]), path))
    else:
        variant, out = sys.argv[2], sys.argv[3]
        parts = build(model, variant)
        parts["variant"] = variant
        data = file_bytes(parts, tok_spec)
        open(out, "wb").write(data)
        print(f"wrote {out}: {len(data) / 1e6:.1f} MB, sha256 {hashlib.sha256(data).hexdigest()}")
        pickle.dump(parts, open(out + ".parts.pkl", "wb"))


if __name__ == "__main__":
    main()
