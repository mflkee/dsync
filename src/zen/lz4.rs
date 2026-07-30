use std::path::Path;

use anyhow::{Context, Result};

const MOZLZ40_MAGIC: &[u8; 8] = b"mozLz40\0";

pub fn read_mozlz4(path: &Path) -> Result<serde_json::Value> {
    let data = std::fs::read(path)
        .with_context(|| format!("reading {}", path.display()))?;

    if data.len() < 12 || &data[..8] != MOZLZ40_MAGIC {
        anyhow::bail!("not a mozlz4 file: {}", path.display());
    }

    let uncompressed_size = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;

    let decompressed = lz4_flex::decompress(&data[12..], uncompressed_size)
        .context("lz4 decompression failed")?;

    let value: serde_json::Value = serde_json::from_slice(&decompressed)
        .context("json parse failed")?;

    Ok(value)
}

pub fn write_mozlz4(path: &Path, value: &serde_json::Value) -> Result<()> {
    let json_bytes = serde_json::to_vec(value).context("json serialize failed")?;
    let compressed = lz4_flex::compress(&json_bytes);

    let uncompressed_size = (json_bytes.len() as u32).to_le_bytes();
    let mut output = Vec::with_capacity(12 + compressed.len());
    output.extend_from_slice(MOZLZ40_MAGIC);
    output.extend_from_slice(&uncompressed_size);
    output.extend_from_slice(&compressed);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("creating parent directories")?;
    }

    std::fs::write(path, &output)
        .with_context(|| format!("writing {}", path.display()))?;

    Ok(())
}
