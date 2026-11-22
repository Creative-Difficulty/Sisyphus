# Models

`sisyphus-prompt-1.bin`: the local prompt classifier (about 27 MB). The app
downloads it from the Sisyphus server on first start ("Additional required
files") and only uses it if its SHA-256 matches the one built into the app
(`crates/client/src/prompt_model/mod.rs`). The server embeds this file and
serves it at `/models/sisyphus-prompt-1.bin`.

It is a fine-tuned, compressed version of
[minishlab/blink-multilingual-64M](https://huggingface.co/minishlab/blink-multilingual-64M)
(MIT, see `LICENSE-blink-multilingual-64M.txt`) with a small classifier on
top. How it was trained, and how to rebuild it: `tools/prompt-model/`.
