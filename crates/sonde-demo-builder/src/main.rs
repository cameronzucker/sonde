//! sonde-demo-builder: turns an operator image into the fixed SITREP
//! `payload.bin` + `payload.offsets.json` consumed by the WASM demo engine.

mod image_fit;
mod sitrep;

fn main() -> anyhow::Result<()> {
    eprintln!("sonde-demo-builder: see `--help` (implemented in later tasks)");
    Ok(())
}
