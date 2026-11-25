//! Model asset manifest parsing and integrity verification.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelAsset {
    pub name: String,
    pub repo: String,
    pub filename: String,
    pub sha256: Option<String>,
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelManifest {
    pub assets: Vec<ModelAsset>,
}

impl ModelManifest {
    pub fn load() -> Self {
        let content = include_str!("../models.toml");
        toml::from_str(content).expect("embedded models.toml must parse")
    }

    pub fn find_asset(&self, filename: &str) -> Option<&ModelAsset> {
        self.assets
            .iter()
            .find(|a| a.name == filename)
            .or_else(|| self.assets.iter().find(|a| a.filename == filename))
    }
}

/// Canonical model file integrity verification engine.
///
/// Checks file existence (if `required`), minimum byte size (if `min_bytes`),
/// exact byte size (if `exact_bytes`), and SHA-256 digest (if `expected_sha256`).
pub fn verify_file_integrity(
    path: &Path,
    filename: &str,
    required: bool,
    min_bytes: Option<u64>,
    exact_bytes: Option<u64>,
    expected_sha256: Option<&str>,
) -> anyhow::Result<()> {
    if !path.exists() {
        if required {
            return Err(anyhow::anyhow!(
                "missing file {:?} (path: {})",
                filename,
                path.display()
            ));
        }
        return Ok(());
    }

    let actual_bytes = std::fs::metadata(path)
        .map_err(|e| {
            anyhow::anyhow!(
                "cannot read metadata for {:?} (path: {}): {e}",
                filename,
                path.display()
            )
        })?
        .len();

    if let Some(min) = min_bytes {
        if actual_bytes < min {
            return Err(anyhow::anyhow!(
                "{:?} is smaller than min bytes ({} < {})",
                filename,
                actual_bytes,
                min
            ));
        }
    }

    if let Some(expected_bytes) = exact_bytes {
        if actual_bytes != expected_bytes {
            return Err(anyhow::anyhow!(
                "Model asset byte length mismatch for '{}' (path: {}): expected {} bytes, found {} bytes",
                filename,
                path.display(),
                expected_bytes,
                actual_bytes
            ));
        }
    }

    if let Some(expected_sha256) = expected_sha256 {
        if !expected_sha256.is_empty() {
            use sha2::{Digest, Sha256};
            let file = std::fs::File::open(path)?;
            let mut hasher = Sha256::new();
            let mut reader = std::io::BufReader::new(file);
            let mut buf = [0u8; 8192];
            loop {
                let n = std::io::Read::read(&mut reader, &mut buf)?;
                if n == 0 {
                    break;
                }
                Digest::update(&mut hasher, &buf[..n]);
            }
            let hash_bytes = hasher.finalize();
            let actual_sha256 = hex::encode(hash_bytes);
            if actual_sha256 != expected_sha256 {
                return Err(anyhow::anyhow!(
                    "Model asset SHA-256 hash mismatch for '{}' (path: {}): expected '{}', found '{}'",
                    filename,
                    path.display(),
                    expected_sha256,
                    actual_sha256
                ));
            }
        }
    }

    Ok(())
}

/// Verify a model asset file against a supplied `ModelManifest`.
/// If unpinned or absent from the manifest, passes without error. Fails fast on mismatch.
pub fn verify_asset_with_manifest(
    path: &Path,
    filename: &str,
    manifest: &ModelManifest,
) -> anyhow::Result<()> {
    if !path.exists() {
        return Ok(());
    }

    let Some(asset) = manifest.find_asset(filename) else {
        return Ok(());
    };

    verify_file_integrity(
        path,
        filename,
        false,
        None,
        asset.bytes,
        asset.sha256.as_deref(),
    )
}

/// Verify a model asset file against pinned bytes and/or sha256 in models.toml.
/// If unpinned, passes without error. Fails fast with explicit error on mismatch.
pub fn verify_asset(path: &Path, filename: &str) -> anyhow::Result<()> {
    verify_asset_with_manifest(path, filename, &ModelManifest::load())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_embedded_manifest_has_pinned_tokenizer_and_unpinned_models() {
        let manifest = ModelManifest::load();
        let tok = manifest
            .find_asset("tokenizer.json")
            .expect("tokenizer.json in models.toml");
        assert_eq!(
            tok.sha256.as_deref(),
            Some("6710678b12670bc442b99edc952c4d996ae309a7020c1fa0096dd245c2faf790")
        );
        assert!(tok.bytes.is_none());

        for unpinned in ["model.onnx", "model_fp16.onnx", "model_quantized.onnx", "reranker.onnx"] {
            let asset = manifest
                .find_asset(unpinned)
                .unwrap_or_else(|| panic!("{unpinned} should be in models.toml"));
            assert!(asset.sha256.is_none(), "{unpinned} must remain unpinned");
            assert!(asset.bytes.is_none(), "{unpinned} must have no fabricated bytes");
        }
    }

    #[test]
    fn test_verify_asset_matching_and_mismatches() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("tokenizer.json");
        std::fs::write(&file_path, b"test content").unwrap();

        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"test content");
        let hash = hex::encode(hasher.finalize());

        let manifest = ModelManifest {
            assets: vec![ModelAsset {
                name: "tokenizer.json".to_string(),
                repo: "test".to_string(),
                filename: "tokenizer.json".to_string(),
                sha256: Some(hash),
                bytes: Some(12),
            }],
        };

        assert!(verify_asset_with_manifest(&file_path, "tokenizer.json", &manifest).is_ok());
        assert!(verify_asset_with_manifest(&file_path, "nonexistent.json", &manifest).is_ok());

        // Real embedded manifest rejects corrupt tokenizer.json
        let err = verify_asset(&file_path, "tokenizer.json")
            .expect_err("corrupt tokenizer.json must fail sha256 verification")
            .to_string();
        assert!(err.contains("SHA-256 hash mismatch"), "unexpected error: {err}");

        // Custom manifest sha256 mismatch
        let bad_hash_manifest = ModelManifest {
            assets: vec![ModelAsset {
                name: "tokenizer.json".to_string(),
                repo: "test".to_string(),
                filename: "tokenizer.json".to_string(),
                sha256: Some("0000".to_string()),
                bytes: Some(12),
            }],
        };
        assert!(verify_asset_with_manifest(&file_path, "tokenizer.json", &bad_hash_manifest).is_err());
    }

    #[test]
    fn test_verify_asset_byte_mismatch_fails() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("model.bin");
        std::fs::write(&file_path, b"short").unwrap();

        let manifest = ModelManifest {
            assets: vec![ModelAsset {
                name: "model.bin".to_string(),
                repo: "test".to_string(),
                filename: "model.bin".to_string(),
                sha256: None,
                bytes: Some(100),
            }],
        };

        let err = verify_asset_with_manifest(&file_path, "model.bin", &manifest)
            .expect_err("byte mismatch must fail")
            .to_string();
        assert!(err.contains("byte length mismatch"), "unexpected error: {err}");
    }
}
