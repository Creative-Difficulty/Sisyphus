"""Write the Rust parity cases: synthetic test prompts plus tokenizer edge
cases, with the Hugging Face token ids and the reference probabilities.

usage: parity.py MODEL.bin OUT.json   (after export.py write, which leaves
MODEL.bin.parts.pkl next to the model)
"""

import json
import pickle
import random
import sys

from tokenizers import Tokenizer

import export
import round4

EDGE_CASES = [
    "", " ", "\t\n", "[UNK]", "[PAD] hi [UNK]", "a" * 150, "x" * 99 + " " + "y" * 101,
    "一个人一个人", "我想要一个按钮", "年会費ティアード", "ボタンの色をピンクにして", "لماذا لا يعمل الكود؟",
    "hello\u0000world�!", "emoji 🎉🔥 test 👍🏽", "zero​width‍joiner", "Ǆemo İstanbul ΣΊΣΥΦΟΣ",
    "été café", "tab\tsep\rcr", "don't stop, (really)!? ...", "„Anführungszeichen“ «guillemets» 「括弧」",
    "ไม่ทำงาน ช่วยด้วย", "क्या तुम मदद कर सकते हो", "ąęśćż łódź", "a" * 1000, "hello" * 50,
    "fix this\nTypeError: Cannot read properties of undefined (reading 'map')\n    at App.jsx:12:5",
    "ＦＵＬＬ　ＷＩＤＴＨ　ｔｅｘｔ", "Straße GROSS ẞ", " line para", "𠀀𠀁 rare cjk", "x̴̡̛ zalgo̷",
]


def main():
    model_path, out = sys.argv[1], sys.argv[2]
    parts = pickle.load(open(model_path + ".parts.pkl", "rb"))
    tok = Tokenizer.from_file(str(round4.ROOT / "models" / round4.BASE / "tokenizer.json"))
    test = [json.loads(l)["text"] for l in open("data/test.jsonl")]
    typo = [json.loads(l)["text"] for l in open("data/test-typos.jsonl")]
    rng = random.Random(1)
    texts = rng.sample(test, 240) + rng.sample(typo, 120) + EDGE_CASES
    probs = export.reference(parts, tok, texts)
    ids = [e.ids for e in tok.encode_batch(texts, add_special_tokens=False)]
    cases = [{"text": t, "ids": i, "probs": [round(float(x), 6) for x in p]} for t, i, p in zip(texts, ids, probs)]
    json.dump({"model": export.NAME, "cases": cases}, open(out, "w"), ensure_ascii=False)
    print(len(cases), "cases")


if __name__ == "__main__":
    main()
