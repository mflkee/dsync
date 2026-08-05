use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use dsync::zen::lz4;

#[derive(Parser)]
struct Args {
    file: PathBuf,
    /// workspace names to drop (default: Space)
    #[arg(long, default_value = "Space")]
    name: Vec<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut value = lz4::read_mozlz4(&args.file)?;

    if let Some(arr) = value.get_mut("spaces").and_then(|v| v.as_array_mut()) {
        let before = arr.len();
        arr.retain(|s| {
            !s.get("name")
                .and_then(|n| n.as_str())
                .is_some_and(|n| args.name.iter().any(|w| w == n))
        });
        println!("spaces: {before} -> {}", arr.len());
    }

    lz4::write_mozlz4(&args.file, &value)?;
    println!("written {}", args.file.display());
    Ok(())
}
