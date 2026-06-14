//! sonde-demo-builder: image -> SITREP `payload.bin` + `payload.offsets.json`.
//!
//! Usage:
//!   sonde-demo-builder <IMAGE> <OUT_DIR> [--target-bytes N] [--max-dim D] [--callsign C]

mod image_fit;
mod sitrep;

use anyhow::{Context, Result};
use std::path::PathBuf;

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let positionals: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if positionals.len() < 2 {
        eprintln!("usage: sonde-demo-builder <IMAGE> <OUT_DIR> [--target-bytes N] [--max-dim D] [--callsign C]");
        std::process::exit(2);
    }
    let image_path = PathBuf::from(positionals[0]);
    let out_dir = PathBuf::from(positionals[1]);
    let target_bytes: usize = arg_value(&args, "--target-bytes")
        .and_then(|v| v.parse().ok())
        .unwrap_or(5000);
    let max_dim: u32 = arg_value(&args, "--max-dim")
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let callsign = arg_value(&args, "--callsign").unwrap_or_else(|| "KK6XYZ".to_string());

    let img =
        image::open(&image_path).with_context(|| format!("opening {}", image_path.display()))?;
    let jpeg = image_fit::fit_jpeg(&img, max_dim, target_bytes)?;

    let position_line = "Position: 34-12.34N / 118-29.10W (DM04xf)";
    let body = "Aerial recon of flood zone: levee breach at N bank, water across Route 9, \
                two structures isolated. No casualties observed. Requesting boat team + \
                medical standby. Photo attached.";
    let (bytes, offsets) = sitrep::build_payload(&callsign, position_line, body, &jpeg);

    std::fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    std::fs::write(out_dir.join("payload.bin"), &bytes)?;
    std::fs::write(
        out_dir.join("payload.offsets.json"),
        serde_json::to_vec_pretty(&offsets)?,
    )?;

    eprintln!(
        "wrote {} byte payload ({} byte image) to {}",
        bytes.len(),
        jpeg.len(),
        out_dir.display()
    );
    Ok(())
}
