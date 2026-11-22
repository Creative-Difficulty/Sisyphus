"""Make realistic typos in prompts, for training data augmentation.

Applies to the human words only: anything after the first line break (pasted
errors, code) stays as it is.
"""

import random
import unicodedata

QWERTY = [
    "1234567890",
    "qwertyuiop",
    "asdfghjkl",
    "zxcvbnm",
]
NEIGHBOURS = {}
for r, row in enumerate(QWERTY):
    for c, ch in enumerate(row):
        near = []
        for dr in (-1, 0, 1):
            if 0 <= r + dr < len(QWERTY):
                other = QWERTY[r + dr]
                for dc in (-1, 0, 1):
                    if (dr or dc) and 0 <= c + dc < len(other):
                        near.append(other[c + dc])
        NEIGHBOURS[ch] = near


def strip_accents(text):
    """Drop accents from Latin and Greek letters only (in other scripts the
    combining marks are part of the letter, like Japanese dakuten)."""
    out = []
    for c in text:
        if c.isalpha() and ord(c) < 0x0400:
            c = "".join(d for d in unicodedata.normalize("NFD", c) if not unicodedata.combining(d))
        out.append(c)
    return "".join(out)


def char_typo(text, rng):
    """One typo at a random letter."""
    positions = [i for i, c in enumerate(text) if c.isalpha()]
    if not positions:
        return text
    i = rng.choice(positions)
    kind = rng.random()
    if kind < 0.3 and i + 1 < len(text) and text[i + 1].isalpha():  # swap
        return text[:i] + text[i + 1] + text[i] + text[i + 2:]
    if kind < 0.55:  # drop
        return text[:i] + text[i + 1:]
    if kind < 0.7:  # double
        return text[:i] + text[i] + text[i:]
    near = NEIGHBOURS.get(text[i].lower())
    if near:  # neighbouring key
        return text[:i] + rng.choice(near) + text[i + 1:]
    return text[:i] + text[i + 1:]


def add_typos(text, rng, rate=1 / 14):
    head, sep, rest = text.partition("\n")
    letters = sum(c.isalpha() for c in head)
    n = sum(rng.random() < rate for _ in range(letters))
    n = min(max(n, 1 if letters >= 4 else 0), 6)
    for _ in range(n):
        head = char_typo(head, rng)
    if rng.random() < 0.3:
        head = strip_accents(head)
    if rng.random() < 0.4:
        head = head.lower()
    if rng.random() < 0.25:
        head = head.rstrip(".!?。！？")
    if rng.random() < 0.15 and " " in head:
        spaces = [i for i, c in enumerate(head) if c == " "]
        i = rng.choice(spaces)
        head = head[:i] + ("  " if rng.random() < 0.3 else "") + head[i + 1:]
    return head + sep + rest


def augment(rows, copies=1, seed=0):
    rng = random.Random(seed)
    out = []
    for r in rows:
        for _ in range(copies):
            noisy = add_typos(r["text"], rng)
            if noisy != r["text"]:
                out.append({**r, "text": noisy})
    return out


if __name__ == "__main__":
    rng = random.Random(1)
    for t in ["kannst du den login button fixen, der geht nicht mehr",
              "Warum funktioniert meine Schleife nicht?",
              "ボタンの色をピンクにして",
              "fix this pls\nTypeError: Cannot read properties of undefined (reading 'map')",
              "почему не работает кнопка"]:
        print(repr(add_typos(t, rng)))
