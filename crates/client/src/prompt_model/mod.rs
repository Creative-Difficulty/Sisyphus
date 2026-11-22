//! The local prompt classifier (decided 2026-11-22): gives every prompt a
//! probability from 0.0 to 1.0 for each category (what kind of message it
//! is, what it's about). Only these numbers leave the machine, never the
//! prompt.
//!
//! The model is a fine-tuned static embedding table (from
//! `minishlab/blink-multilingual-64M`, MIT) with a small classifier on top,
//! about 27 MB. It isn't bundled with the app: the first start downloads it
//! from the Sisyphus server ("Additional required files") into the config
//! folder, and it is only used if its SHA-256 matches [`MODEL_SHA256`], so
//! the server can't hand out a different model.
//!
//! File format (little endian): `SPMODEL1`, a u32 length and a JSON header,
//! then sections, each a u32 byte length and the bytes: the WordPiece
//! vocabulary, the added tokens, the punctuation and "other" Unicode ranges,
//! the product-quantization codebooks and codes of the embedding table, the
//! fine-tuned rows (ids, per-dimension scale, int8 values), then per layer
//! a per-row scale, int8 weights and the bias. The training scripts are in
//! `tools/prompt-model/`.
//!
//! Inference: tokenize, drop `[UNK]`, average the rows, L2-normalize, run
//! the layers (ReLU between them), sigmoid.
//!
//! Memory: the file stays open and only the rows a prompt uses are read
//! from it (a few kilobytes per prompt). Kept in memory are the word index
//! (about 5 MB), the codebooks and the layers.

mod tokenizer;

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use sha2::{Digest, Sha256};

use tokenizer::{StrIndex, Tokenizer};

/// The model's name, sent in heartbeats next to its outputs.
pub const MODEL_NAME: &str = "sisyphus-prompt-1";
pub const MODEL_FILE: &str = "sisyphus-prompt-1.bin";
pub const MODEL_SHA256: &str = "87a54ae84d61938c8a637a9bcb0bae4f7927404609cb0cd6e3a91aa16595393d";
pub const MODEL_SIZE: u64 = 26_651_435;

/// The categories, in output order: 9 acts (what kind of message), then 13
/// tasks (what it's about). Must match the model file's header.
pub const LABELS: [&str; 22] = [
    "act:new_request",
    "act:correction",
    "act:clarification",
    "act:approval",
    "act:question",
    "act:stop_undo",
    "act:handoff",
    "act:chit_chat",
    "act:other",
    "task:feature",
    "task:bug_fix",
    "task:error_paste",
    "task:refactor",
    "task:understand_code",
    "task:learn_concept",
    "task:ui_styling",
    "task:tests",
    "task:setup_deploy",
    "task:docs_writing",
    "task:planning_design",
    "task:review",
    "task:other",
];

/// Index of `act:correction` in [`LABELS`].
pub const CORRECTION: usize = 1;

pub struct PromptModel {
    tokenizer: Tokenizer,
    dim: usize,
    subspaces: usize,
    vocab_size: usize,
    /// [subspaces][256][dim / subspaces]
    books: Vec<f32>,
    /// The model file. Rows are read from it when a prompt needs them.
    file: Mutex<File>,
    /// Where the PQ codes start: [vocab][subspaces] bytes.
    codes_at: u64,
    /// Token ids whose rows were fine-tuned, sorted; their int8 rows
    /// ([tuned][dim]) start at `tuned_at`.
    tuned_ids: Vec<u32>,
    tuned_scale: Vec<f32>,
    tuned_at: u64,
    layers: Vec<Layer>,
}

struct Layer {
    inputs: usize,
    outputs: usize,
    scale: Vec<f32>,
    weights: Vec<i8>,
    bias: Vec<f32>,
}

/// Where the model is stored: `models/` next to the config.
pub fn path() -> Option<PathBuf> {
    Some(
        crate::config::Config::path()?
            .parent()?
            .join("models")
            .join(MODEL_FILE),
    )
}

/// The model file is there with the right size (its hash is checked when
/// it's loaded).
pub fn is_installed() -> bool {
    path()
        .and_then(|p| fs::metadata(p).ok())
        .is_some_and(|m| m.len() == MODEL_SIZE)
}

impl PromptModel {
    /// Open the installed model, if it's there and unmodified.
    pub fn load_installed() -> Result<Self, String> {
        Self::open(&path().ok_or("No config folder")?)
    }

    /// Open a model file. Refuses anything but the expected file.
    pub fn open(path: &Path) -> Result<Self, String> {
        let mut file = File::open(path).map_err(|e| format!("Could not read the model: {e}"))?;
        let damaged = || "The model file is damaged or not the expected version".to_string();
        if !matches_hash(&mut file).map_err(|e| format!("Could not read the model: {e}"))? {
            return Err(damaged());
        }
        file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        Self::parse(file).ok_or_else(damaged)
    }

    fn parse(file: File) -> Option<Self> {
        let mut r = FileReader { file, at: 0 };
        if r.take(8)? != b"SPMODEL1" {
            return None;
        }
        let header_len = r.u32()? as usize;
        let header: serde_json::Value = serde_json::from_slice(&r.take(header_len)?).ok()?;
        let labels: Vec<&str> = header["labels"]
            .as_array()?
            .iter()
            .filter_map(|l| l.as_str())
            .collect();
        if labels != LABELS {
            return None;
        }
        let num = |key: &str| header[key].as_u64().map(|n| n as usize);
        let dim = num("dim")?;
        let subspaces = num("subspaces")?;
        let vocab_size = num("vocab_size")?;
        let tuned_rows = num("tuned_rows")?;
        if header["tuned_format"] != "int8" || dim % subspaces != 0 {
            return None;
        }

        let vocab = String::from_utf8(r.section()?).ok()?;
        let vocab = StrIndex::new(vocab.split('\n').zip(0u32..));
        let added_text = String::from_utf8(r.section()?).ok()?;
        let mut added = Vec::new();
        for line in added_text.split('\n') {
            let (id, content) = line.split_once('\t')?;
            added.push((content, id.parse().ok()?));
        }
        let added_max_chars = added
            .iter()
            .map(|(k, _)| k.chars().count())
            .max()
            .unwrap_or(0);
        let added = StrIndex::new(added.into_iter());
        let punctuation = ranges(&r.section()?)?;
        let other = ranges(&r.section()?)?;
        let books = f32s(&r.section()?);
        let (codes_at, codes_len) = r.skip_section()?;
        let tuned_ids: Vec<u32> = r
            .section()?
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        let (tuned_section_at, tuned_len) = r.skip_section()?;
        if books.len() != subspaces * 256 * (dim / subspaces)
            || codes_len != (vocab_size * subspaces) as u64
            || tuned_ids.len() != tuned_rows
            || !tuned_ids.is_sorted()
            || tuned_len != (dim * 4 + tuned_rows * dim) as u64
        {
            return None;
        }

        let mut layers = Vec::new();
        for shape in header["layers"].as_array()? {
            let inputs = shape[0].as_u64()? as usize;
            let outputs = shape[1].as_u64()? as usize;
            let scale = f32s(&r.section()?);
            let weights: Vec<i8> = r.section()?.into_iter().map(|b| b as i8).collect();
            let bias = f32s(&r.section()?);
            if scale.len() != outputs || weights.len() != inputs * outputs || bias.len() != outputs
            {
                return None;
            }
            layers.push(Layer {
                inputs,
                outputs,
                scale,
                weights,
                bias,
            });
        }
        if layers.first()?.inputs != dim || layers.last()?.outputs != LABELS.len() {
            return None;
        }
        // The tuned rows' per-dimension scale sits at the start of their
        // section.
        let mut scale = vec![0u8; dim * 4];
        r.file.seek(SeekFrom::Start(tuned_section_at)).ok()?;
        r.file.read_exact(&mut scale).ok()?;

        let tokenizer = Tokenizer {
            vocab,
            added,
            added_max_chars,
            punctuation,
            other,
            unk_id: num("unk_id")? as u32,
            max_input_chars_per_word: num("max_input_chars_per_word")?,
            fixed_length: num("fixed_length")?,
        };
        Some(Self {
            tokenizer,
            dim,
            subspaces,
            vocab_size,
            books,
            file: Mutex::new(r.file),
            codes_at,
            tuned_ids,
            tuned_scale: f32s(&scale),
            tuned_at: tuned_section_at + (dim * 4) as u64,
            layers,
        })
    }

    /// The probability of each category in [`LABELS`], 0.0 to 1.0. `None`
    /// if the model file can't be read anymore.
    pub fn classify(&self, text: &str) -> Option<[f32; 22]> {
        let used: Vec<u32> = self
            .token_ids(text)
            .into_iter()
            .filter(|&id| (id as usize) < self.vocab_size)
            .collect();
        let mut x = vec![0f32; self.dim];
        {
            let mut file = self.file.lock().ok()?;
            let mut row = vec![0u8; self.dim];
            for &id in &used {
                self.add_row(&mut file, id, &mut row, &mut x)?;
            }
        }
        if !used.is_empty() {
            let n = used.len() as f32;
            x.iter_mut().for_each(|v| *v /= n);
        }
        let norm = x.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
        x.iter_mut().for_each(|v| *v /= norm);
        for (i, layer) in self.layers.iter().enumerate() {
            x = layer.forward(&x);
            if i + 1 < self.layers.len() {
                x.iter_mut().for_each(|v| *v = v.max(0.0));
            }
        }
        let mut out = [0f32; 22];
        for (o, v) in out.iter_mut().zip(x) {
            *o = 1.0 / (1.0 + (-v).exp());
        }
        Some(out)
    }

    /// Token ids without `[UNK]`.
    pub fn token_ids(&self, text: &str) -> Vec<u32> {
        let unk = self.tokenizer.unk_id;
        self.tokenizer
            .encode(text)
            .into_iter()
            .filter(|&id| id != unk)
            .collect()
    }

    /// Add token `id`'s row to `x`, read from the file (`buf` is scratch
    /// space of `dim` bytes).
    fn add_row(&self, file: &mut File, id: u32, buf: &mut [u8], x: &mut [f32]) -> Option<()> {
        if let Ok(row) = self.tuned_ids.binary_search(&id) {
            file.seek(SeekFrom::Start(self.tuned_at + (row * self.dim) as u64))
                .ok()?;
            file.read_exact(&mut buf[..self.dim]).ok()?;
            for ((x, &q), s) in x.iter_mut().zip(buf.iter()).zip(&self.tuned_scale) {
                *x += q as i8 as f32 * s;
            }
            return Some(());
        }
        let codes = &mut buf[..self.subspaces];
        file.seek(SeekFrom::Start(
            self.codes_at + (id as usize * self.subspaces) as u64,
        ))
        .ok()?;
        file.read_exact(codes).ok()?;
        let sub = self.dim / self.subspaces;
        for (j, &code) in codes.iter().enumerate() {
            let book =
                &self.books[(j * 256 + code as usize) * sub..(j * 256 + code as usize + 1) * sub];
            for (x, b) in x[j * sub..(j + 1) * sub].iter_mut().zip(book) {
                *x += b;
            }
        }
        Some(())
    }
}

impl Layer {
    fn forward(&self, x: &[f32]) -> Vec<f32> {
        (0..self.outputs)
            .map(|o| {
                let row = &self.weights[o * self.inputs..(o + 1) * self.inputs];
                let dot: f32 = row.iter().zip(x).map(|(&w, &v)| w as f32 * v).sum();
                dot * self.scale[o] + self.bias[o]
            })
            .collect()
    }
}

/// Probabilities as sent in heartbeats: whole numbers from 0 to 1000.
pub fn per_mille(probs: &[f32; 22]) -> [u16; 22] {
    probs.map(|p| (p.clamp(0.0, 1.0) * 1000.0).round() as u16)
}

/// The file is exactly the expected model (size and SHA-256), read in
/// pieces so it never sits in memory whole.
fn matches_hash(file: &mut File) -> std::io::Result<bool> {
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        total += n as u64;
    }
    Ok(total == MODEL_SIZE && hex::encode(hash.finalize()) == MODEL_SHA256)
}

/// Reads the model file's sections in order.
struct FileReader {
    file: File,
    at: u64,
}

impl FileReader {
    fn take(&mut self, n: usize) -> Option<Vec<u8>> {
        if self.at + n as u64 > MODEL_SIZE {
            return None;
        }
        let mut out = vec![0u8; n];
        self.file.read_exact(&mut out).ok()?;
        self.at += n as u64;
        Some(out)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn section(&mut self) -> Option<Vec<u8>> {
        let n = self.u32()? as usize;
        self.take(n)
    }

    /// Skip a section without reading it: its start and length.
    fn skip_section(&mut self) -> Option<(u64, u64)> {
        let n = self.u32()? as u64;
        let start = self.at;
        if start + n > MODEL_SIZE {
            return None;
        }
        self.file.seek(SeekFrom::Current(n as i64)).ok()?;
        self.at += n;
        Some((start, n))
    }
}

fn f32s(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect()
}

fn ranges(bytes: &[u8]) -> Option<Vec<(u32, u32)>> {
    if !bytes.len().is_multiple_of(8) {
        return None;
    }
    Some(
        bytes
            .chunks_exact(8)
            .map(|b| {
                (
                    u32::from_le_bytes(b[..4].try_into().unwrap()),
                    u32::from_le_bytes(b[4..].try_into().unwrap()),
                )
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Download

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Download the model from `server` into the config folder. `progress` gets
/// (bytes so far, total). The file is kept only if its SHA-256 matches.
/// Blocking; run it off the UI thread.
pub fn download(server: &str, mut progress: impl FnMut(u64, u64)) -> Result<(), String> {
    let server = server.trim_end_matches('/');
    if !crate::login::is_safe_server(server) {
        return Err("The server address must use https.".to_string());
    }
    let target = path().ok_or("No config folder")?;
    let dir = target.parent().ok_or("No config folder")?;
    fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let http: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(DOWNLOAD_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = http
        .get(&format!("{server}/models/{MODEL_FILE}"))
        .call()
        .map_err(|e| format!("Could not reach the server: {e}"))?;
    if response.status() != 200 {
        return Err(format!(
            "The server answered {}.",
            response.status().as_u16()
        ));
    }
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(MODEL_SIZE + 1)
        .reader();
    let partial = target.with_extension("part");
    let mut file =
        fs::File::create(&partial).map_err(|e| format!("Could not save the files: {e}"))?;
    let mut hash = Sha256::new();
    let mut done = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    progress(0, MODEL_SIZE);
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("Download interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(|e| format!("Could not save the files: {e}"))?;
        done += n as u64;
        progress(done, MODEL_SIZE);
    }
    drop(file);
    if done != MODEL_SIZE || hex::encode(hash.finalize()) != MODEL_SHA256 {
        let _ = fs::remove_file(&partial);
        return Err("The downloaded files are damaged. Please try again.".to_string());
    }
    fs::rename(&partial, &target).map_err(|e| format!("Could not save the files: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests;
