//! Checks the Rust model against the Python reference: the token ids must
//! match the Hugging Face tokenizer exactly, the probabilities to 0.0005.
//! The cases are synthetic test prompts plus tokenizer edge cases.

use super::*;

fn model() -> PromptModel {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../models/sisyphus-prompt-1.bin"
    );
    PromptModel::open(Path::new(path)).expect("models/sisyphus-prompt-1.bin loads")
}

#[test]
fn matches_the_reference() {
    let model = model();
    let cases: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/prompt_model_parity.json"
    )))
    .unwrap();
    let mut id_mismatches = Vec::new();
    let mut worst = 0f32;
    for case in cases["cases"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        let expected: Vec<u32> = case["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u32)
            .collect();
        let ids = model.tokenizer.encode(text);
        if ids != expected {
            id_mismatches.push(format!(
                "{text:?}\n  got      {ids:?}\n  expected {expected:?}"
            ));
            continue;
        }
        let probs = model.classify(text).expect("model file readable");
        for (p, e) in probs.iter().zip(case["probs"].as_array().unwrap()) {
            worst = worst.max((p - e.as_f64().unwrap() as f32).abs());
        }
    }
    assert!(
        id_mismatches.is_empty(),
        "{} token mismatches:\n{}",
        id_mismatches.len(),
        id_mismatches.join("\n")
    );
    assert!(worst < 5e-4, "largest probability difference {worst}");
}

#[test]
fn refuses_other_files() {
    let path = std::env::temp_dir().join(format!("sisyphus-not-a-model-{}", std::process::id()));
    fs::write(&path, b"SPMODEL1 not really").unwrap();
    assert!(PromptModel::open(&path).is_err());
    let _ = fs::remove_file(&path);
}

#[test]
fn per_mille_rounds_and_clamps() {
    let mut p = [0f32; 22];
    p[0] = 0.9996;
    p[1] = 0.0004;
    p[2] = 1.5;
    let q = per_mille(&p);
    assert_eq!((q[0], q[1], q[2]), (1000, 0, 1000));
}
