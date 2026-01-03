use anyhow::{bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use std::path::{Path, PathBuf};

use crate::config::Config;

/// Embedding dimension for all-MiniLM-L6-v2
pub const EMBEDDING_DIM: usize = 384;

const MODEL_REPO: &str = "sentence-transformers/all-MiniLM-L6-v2";
const TOKENIZER_FILE: &str = "tokenizer.json";

/// Pick the best quantized ONNX model for the current CPU.
/// Falls back to optimized float32 if no quantized variant matches.
fn best_model_for_platform() -> (&'static str, &'static str) {
    #[cfg(target_arch = "aarch64")]
    {
        return ("model_qint8_arm64.onnx", "ARM64 INT8 quantized");
    }

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx512f") {
            return ("model_qint8_avx512.onnx", "AVX-512 INT8 quantized");
        }
        if is_x86_feature_detected!("avx2") {
            return ("model_quint8_avx2.onnx", "AVX2 UINT8 quantized");
        }
    }

    // Fallback: optimized float32 (works everywhere, still faster than base)
    #[allow(unreachable_code)]
    ("model_O4.onnx", "optimized float32")
}

/// Local embedding engine using all-MiniLM-L6-v2 via ONNX Runtime.
/// No API calls, no data leaves your machine.
pub struct Embedder {
    session: Session,
    tokenizer: tokenizers::Tokenizer,
}

impl Embedder {
    /// Load (or download on first use) the embedding model.
    pub fn load() -> Result<Self> {
        let model_dir = Self::ensure_model_downloaded()?;

        let (model_file, _) = best_model_for_platform();
        let model_path = model_dir.join(model_file);
        let tokenizer_path = model_dir.join(TOKENIZER_FILE);

        tracing::info!("Loading ONNX model from {}", model_path.display());

        // Use half the available cores so the OS stays responsive
        let num_cores = std::thread::available_parallelism()
            .map(|n| (n.get() / 2).max(2))
            .unwrap_or(4);

        let session = Session::builder()
            .and_then(|b| b.with_optimization_level(GraphOptimizationLevel::Level3))
            .and_then(|b| b.with_intra_threads(num_cores))
            .and_then(|b| b.commit_from_file(&model_path))
            .context("Failed to load ONNX session")?;

        let mut tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

        // Truncate to 128 tokens — captures name+signature+docstring which is
        // all the semantic signal we need.  Keeps batches tight, cuts O(n²)
        // attention cost, and slashes padding waste when texts vary in length.
        use tokenizers::TruncationParams;
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: 128,
                ..Default::default()
            }))
            .map_err(|e| anyhow::anyhow!("Failed to set truncation: {}", e))?;

        Ok(Embedder { session, tokenizer })
    }

    /// Generate an embedding vector for the given text.
    pub fn embed(&mut self, text: &str) -> Result<Vec<f32>> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;

        let input_ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
        let attention_mask: Vec<i64> = encoding
            .get_attention_mask()
            .iter()
            .map(|&m| m as i64)
            .collect();
        let token_type_ids: Vec<i64> = encoding
            .get_type_ids()
            .iter()
            .map(|&t| t as i64)
            .collect();

        let seq_len = input_ids.len();

        // Use tuple (shape, data) format — avoids ndarray version mismatch
        let ids_value = ort::value::Tensor::from_array(([1usize, seq_len], input_ids))?;
        let mask_value = ort::value::Tensor::from_array(([1usize, seq_len], attention_mask.clone()))?;
        let type_value = ort::value::Tensor::from_array(([1usize, seq_len], token_type_ids))?;

        let outputs = self
            .session
            .run(ort::inputs![ids_value, mask_value, type_value])
            .context("ONNX inference failed")?;

        // Output shape: [1, seq_len, 384]
        // Shape derefs to &[i64], copy data to avoid borrow conflict
        let (shape, raw_data) = outputs[0]
            .try_extract_tensor::<f32>()
            .context("Failed to extract tensor")?;
        let shape_vec: Vec<i64> = shape.iter().copied().collect();
        let data_vec: Vec<f32> = raw_data.to_vec();
        drop(outputs);

        // Mean pooling with attention mask
        let embedding = Self::mean_pool_raw(&data_vec, &shape_vec, &attention_mask);

        // L2 normalize
        let normalized = l2_normalize(&embedding);

        Ok(normalized)
    }

    /// Embed multiple texts in a single batched ONNX inference call.
    /// Pads all inputs to max sequence length in the batch.
    /// Returns one embedding vector per input text.
    pub fn embed_batch(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        if texts.len() == 1 {
            return Ok(vec![self.embed(&texts[0])?]);
        }

        // Tokenize all texts
        let encodings: Vec<_> = texts
            .iter()
            .map(|t| {
                self.tokenizer
                    .encode(t.as_str(), true)
                    .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))
            })
            .collect::<Result<Vec<_>>>()?;

        let batch_size = encodings.len();
        let max_len = encodings.iter().map(|e| e.get_ids().len()).max().unwrap_or(0);

        // Build padded flat tensors [batch_size, max_len]
        let mut all_ids = vec![0i64; batch_size * max_len];
        let mut all_mask = vec![0i64; batch_size * max_len];
        let mut all_types = vec![0i64; batch_size * max_len];
        let mut lengths = Vec::with_capacity(batch_size);

        for (i, enc) in encodings.iter().enumerate() {
            let ids = enc.get_ids();
            let mask = enc.get_attention_mask();
            let types = enc.get_type_ids();
            let len = ids.len();
            lengths.push(len);
            let offset = i * max_len;
            for j in 0..len {
                all_ids[offset + j] = ids[j] as i64;
                all_mask[offset + j] = mask[j] as i64;
                all_types[offset + j] = types[j] as i64;
            }
        }

        let ids_tensor = ort::value::Tensor::from_array(([batch_size, max_len], all_ids))?;
        let mask_tensor = ort::value::Tensor::from_array(([batch_size, max_len], all_mask.clone()))?;
        let type_tensor = ort::value::Tensor::from_array(([batch_size, max_len], all_types))?;

        let outputs = self
            .session
            .run(ort::inputs![ids_tensor, mask_tensor, type_tensor])
            .context("Batched ONNX inference failed")?;

        // Output shape: [batch_size, max_len, 384]
        let (shape, raw_data) = outputs[0]
            .try_extract_tensor::<f32>()
            .context("Failed to extract batched tensor")?;
        let _shape_vec: Vec<i64> = shape.iter().copied().collect();
        let data_vec: Vec<f32> = raw_data.to_vec();
        drop(outputs);

        let embed_dim = EMBEDDING_DIM;
        let stride = max_len * embed_dim; // elements per batch item

        let mut results = Vec::with_capacity(batch_size);
        for i in 0..batch_size {
            let item_offset = i * stride;
            let item_data = &data_vec[item_offset..item_offset + stride];
            // Build per-item attention mask
            let item_mask = &all_mask[i * max_len..(i * max_len) + max_len];
            let item_shape = vec![1i64, max_len as i64, embed_dim as i64];
            let pooled = Self::mean_pool_raw(item_data, &item_shape, item_mask);
            results.push(l2_normalize(&pooled));
        }

        Ok(results)
    }

    /// Mean pooling: average token embeddings weighted by attention mask.
    /// Works on raw flat f32 data with shape [1, seq_len, embed_dim].
    fn mean_pool_raw(
        raw_data: &[f32],
        shape: &[i64],
        attention_mask: &[i64],
    ) -> Vec<f32> {
        let seq_len = if shape.len() >= 2 { shape[1] as usize } else { attention_mask.len() };
        let embed_dim = if shape.len() >= 3 { shape[2] as usize } else { EMBEDDING_DIM };
        let mut pooled = vec![0.0f32; embed_dim];
        let mut total_weight: f32 = 0.0;

        for i in 0..seq_len {
            let mask = attention_mask.get(i).copied().unwrap_or(0) as f32;
            if mask > 0.0 {
                let offset = i * embed_dim;
                for j in 0..embed_dim {
                    if let Some(&val) = raw_data.get(offset + j) {
                        pooled[j] += val * mask;
                    }
                }
                total_weight += mask;
            }
        }

        if total_weight > 0.0 {
            for val in &mut pooled {
                *val /= total_weight;
            }
        }
        pooled
    }

    /// Ensure the embedding model is downloaded to the local cache.
    fn ensure_model_downloaded() -> Result<PathBuf> {
        let model_dir = Config::models_dir().join("all-MiniLM-L6-v2");
        let (model_file, model_desc) = best_model_for_platform();
        let model_path = model_dir.join(model_file);
        let tokenizer_path = model_dir.join(TOKENIZER_FILE);

        if model_path.exists() && tokenizer_path.exists() {
            return Ok(model_dir);
        }

        std::fs::create_dir_all(&model_dir)?;
        eprintln!("⬇ Downloading {} embedding model (first time only)...", model_desc);

        // Download the platform-optimal quantized model
        if !model_path.exists() {
            let url = format!(
                "https://huggingface.co/{}/resolve/main/onnx/{}",
                MODEL_REPO, model_file
            );
            download_file(&url, &model_path)?;
        }

        // Download tokenizer.json
        if !tokenizer_path.exists() {
            let url = format!(
                "https://huggingface.co/{}/resolve/main/{}",
                MODEL_REPO, TOKENIZER_FILE
            );
            download_file(&url, &tokenizer_path)?;
        }

        eprintln!("✓ Model cached at {}", model_dir.display());
        Ok(model_dir)
    }
}

/// Cosine similarity between two vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// L2-normalize a vector in place, returning the result.
pub fn l2_normalize(v: &[f32]) -> Vec<f32> {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        return v.to_vec();
    }
    v.iter().map(|x| x / norm).collect()
}

fn download_file(url: &str, dest: &Path) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    let resp = client
        .get(url)
        .header("User-Agent", "plex/0.1")
        .send()
        .with_context(|| format!("Failed to download {}", url))?;

    if !resp.status().is_success() {
        bail!("Download failed: HTTP {}", resp.status());
    }

    let total_size = resp.content_length().unwrap_or(0);
    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("  {bar:40.cyan/dim} {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("█▓░"),
    );

    let bytes = resp.bytes()?;
    pb.set_position(bytes.len() as u64);
    pb.finish();

    std::fs::write(dest, &bytes)
        .with_context(|| format!("Failed to write {}", dest.display()))?;

    Ok(())
}

/// Build a text representation of a symbol for embedding.
/// Combines name, signature, docstring, and body for richer embedding.
pub fn symbol_to_embed_text(
    name: &str,
    kind: &str,
    signature: Option<&str>,
    doc_comment: Option<&str>,
    body_snippet: Option<&str>,
) -> String {
    let mut parts = vec![format!("{} {}", kind, name)];
    if let Some(sig) = signature {
        parts.push(sig.to_string());
    }
    if let Some(doc) = doc_comment {
        parts.push(doc.to_string());
    }
    if let Some(body) = body_snippet {
        parts.push(body.to_string());
    }
    parts.join(" | ")
}
