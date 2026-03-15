mod audio;
mod models;
mod openrouter;
mod pipeline;

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

use models::Config;

/// Video Dubber — Automatic video dubbing using AI
///
/// Extracts audio from an MP4 video, transcribes it, translates to Portuguese,
/// and generates a dubbed audio file synchronized with the original speech timing.
#[derive(Parser, Debug)]
#[command(name = "video-dubber", version, about)]
struct Cli {
    /// Path to the input MP4 video file
    #[arg(short, long)]
    input: PathBuf,

    /// Path for the output dubbed video file (MP4) or audio (WAV)
    #[arg(short, long, default_value = "output_dubbed.mp4")]
    output: PathBuf,

    /// TTS voice to use (alloy, echo, fable, onyx, nova, shimmer)
    #[arg(short, long, default_value = "onyx")]
    voice: String,

    /// OpenRouter API key (or set OPENROUTER_API_KEY env var)
    #[arg(long, env = "OPENROUTER_API_KEY")]
    api_key: Option<String>,

    /// Maximum concurrent TTS requests
    #[arg(long, default_value = "5")]
    max_concurrent: usize,

    /// Chunk duration in seconds for audio splitting (default: 300 = 5 min)
    #[arg(long, default_value = "300")]
    chunk_duration: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file if present
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    // Resolve API key
    let api_key = cli
        .api_key
        .or_else(|| std::env::var("OPENROUTER_API_KEY").ok())
        .context(
            "API key not found. Use --api-key or set OPENROUTER_API_KEY environment variable.",
        )?;

    // Validate input file
    if !cli.input.exists() {
        anyhow::bail!("Input file not found: {:?}", cli.input);
    }

    if !cli.input.extension().map_or(false, |ext| ext == "mp4") {
        anyhow::bail!("Input file must be an MP4 video: {:?}", cli.input);
    }

    let mut config = Config::new(api_key, cli.input, cli.output, cli.voice);
    config.max_concurrent_tts = cli.max_concurrent;
    config.chunk_duration_secs = cli.chunk_duration;

    println!("🎬 Video Dubber — Starting dubbing pipeline");
    println!("   Input:  {:?}", config.input_path);
    println!("   Output: {:?}", config.output_path);
    println!("   Voice:  {}", config.voice);
    println!();

    pipeline::run(&config).await?;

    println!();
    println!("✅ Dubbing complete! Output saved to: {:?}", config.output_path);

    Ok(())
}
