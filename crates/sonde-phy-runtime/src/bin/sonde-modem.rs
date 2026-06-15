//! `sonde-modem` — the SondePhy runtime entrypoint (sonde-cyo).
//!
//! Runs the production [`SondePhy`] `PhyTransport` with the full standard
//! waveform ladder (ofdm-wide/mid/narrow + floor-wblo + floor-nfsk) over a real
//! radio — closing the gap where the only real-radio path (`sonde-tx`/`sonde-rx`)
//! bypassed the runtime, so the adaptive multi-mode stack had no on-air entry.
//!
//! ## Modes
//! ```text
//! sonde-modem --loopback --send <TEXT> [--mode <NAME>]
//!     In-process loopback (no hardware): encode + decode through the full
//!     registry and print what came back. A self-test of the runtime + ladder.
//!
//! sonde-modem --send <TEXT> [--mode <NAME>] \
//!             --output-device <DEV> --input-device <DEV> --ptt-device <TTY>
//!     Real radio (built with `--features hardware`): keys PTT + soundcard via
//!     SoundcardRadio. RADIO-1: the licensee runs this; airtime budget + PTT
//!     timing are enforced in sonde-tx's run_transmission.
//! ```
//!
//! Modes (`--mode`): `floor-wblo` (default, robust) · `floor-nfsk` (deep floor) ·
//! `ofdm-narrow` · `ofdm-mid` · `ofdm-wide` (fastest).

use std::process::ExitCode;
use std::time::{Duration, Instant};

use sonde_phy::modes::ModeHint;
use sonde_phy::phy_api::PhyTransport;
use sonde_phy_runtime::{standard_waveforms, SondePhy};

const USAGE: &str = "\
sonde-modem — SondePhy runtime over the full waveform ladder

USAGE:
  sonde-modem --loopback --send <TEXT> [--mode <NAME>]
  sonde-modem --send <TEXT> [--mode <NAME>] \\
              --output-device <DEV> --input-device <DEV> --ptt-device <TTY> \\
              [--max-airtime <SECS>]      # requires build --features hardware

MODES (--mode): floor-wblo (default) | floor-nfsk | ofdm-narrow | ofdm-mid | ofdm-wide
";

/// Map a CLI mode name to a `ModeHint`. `ModeHint::MainPinned` needs a `'static`
/// str, so we match known names to literals rather than leak the CLI string.
fn mode_hint(name: &str) -> Option<ModeHint> {
    Some(match name {
        "floor-wblo" => ModeHint::Floor,
        "floor-nfsk" => ModeHint::FloorCrowdedBand,
        "ofdm-narrow" => ModeHint::MainPinned("ofdm-narrow"),
        "ofdm-mid" => ModeHint::MainPinned("ofdm-mid"),
        "ofdm-wide" => ModeHint::MainPinned("ofdm-wide"),
        _ => return None,
    })
}

struct Args {
    loopback: bool,
    send: Option<String>,
    mode: String,
    output_device: Option<String>,
    input_device: Option<String>,
    ptt_device: Option<String>,
    max_airtime_secs: u64,
    help: bool,
}

fn parse(argv: &[String]) -> Result<Args, String> {
    let mut a = Args {
        loopback: false,
        send: None,
        mode: "floor-wblo".to_string(),
        output_device: None,
        input_device: None,
        ptt_device: None,
        max_airtime_secs: 30,
        help: false,
    };
    // Consume the value token after a flag at index `i`, advancing `i` past it.
    fn take(argv: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
        *i += 1;
        argv.get(*i)
            .cloned()
            .ok_or_else(|| format!("{flag} needs a value"))
    }
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--loopback" => a.loopback = true,
            "--send" => a.send = Some(take(argv, &mut i, "--send")?),
            "--mode" => a.mode = take(argv, &mut i, "--mode")?,
            "--output-device" => a.output_device = Some(take(argv, &mut i, "--output-device")?),
            "--input-device" => a.input_device = Some(take(argv, &mut i, "--input-device")?),
            "--ptt-device" => a.ptt_device = Some(take(argv, &mut i, "--ptt-device")?),
            "--max-airtime" => {
                a.max_airtime_secs = take(argv, &mut i, "--max-airtime")?
                    .parse()
                    .map_err(|_| "bad --max-airtime".to_string())?
            }
            "-h" | "--help" => a.help = true,
            other => return Err(format!("unknown argument: {other}")),
        }
        i += 1;
    }
    Ok(a)
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if args.help || argv.is_empty() {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), String> {
    let hint = mode_hint(&args.mode).ok_or_else(|| format!("unknown --mode {}", args.mode))?;
    let payload = args
        .send
        .clone()
        .ok_or("nothing to do: pass --send <TEXT>")?;

    if args.loopback {
        return run_loopback(&payload, hint, &args.mode);
    }
    run_radio(&payload, hint, &args)
}

/// In-process loopback self-test: the full registry encodes + auto-detects the
/// payload back. No hardware.
fn run_loopback(payload: &str, hint: ModeHint, mode: &str) -> Result<(), String> {
    use sonde_phy_runtime::LoopbackRadio;
    let mut phy = SondePhy::with_waveforms(standard_waveforms(), LoopbackRadio::new());
    phy.send_frame(payload.as_bytes(), hint)
        .map_err(|e| format!("send: {e:?}"))?;
    // Generous timeout: an nFSK over is ~9 s of audio and the pump runs every
    // registered waveform's full sync over the captured window (the cheap detect()
    // pre-gate is not wired yet), so a debug-build decode can take a while.
    let got = poll_one(&mut phy, Duration::from_secs(120)).ok_or("no frame returned")?;
    let text = String::from_utf8_lossy(got.payload());
    println!("loopback [{mode}]: sent {:?}, received {:?}", payload, text);
    if got.payload() != payload.as_bytes() {
        return Err("loopback payload mismatch".into());
    }
    println!("OK ({} → {})", mode, got.mode().short_name());
    phy.shutdown();
    Ok(())
}

#[cfg(feature = "hardware")]
fn run_radio(payload: &str, hint: ModeHint, args: &Args) -> Result<(), String> {
    use sonde_phy_runtime::SoundcardRadio;
    let ptt = args
        .ptt_device
        .as_deref()
        .ok_or("real radio needs --ptt-device <TTY>")?;
    let radio = SoundcardRadio::open(
        args.output_device.as_deref(),
        args.input_device.as_deref(),
        std::path::Path::new(ptt),
        Duration::from_secs(args.max_airtime_secs),
    )
    .map_err(|e| format!("open radio: {e:?}"))?;
    let mut phy = SondePhy::with_waveforms(standard_waveforms(), radio);
    println!("keying {} on [{}] …", payload.len(), args.mode);
    phy.send_frame(payload.as_bytes(), hint)
        .map_err(|e| format!("send: {e:?}"))?;
    // Block until the over is off the air (PTT released).
    let start = Instant::now();
    while phy.tx_in_flight() > 0 && start.elapsed() < Duration::from_secs(args.max_airtime_secs + 5)
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("over complete");
    phy.shutdown();
    Ok(())
}

#[cfg(not(feature = "hardware"))]
fn run_radio(_payload: &str, _hint: ModeHint, _args: &Args) -> Result<(), String> {
    Err(
        "real-radio mode requires building with `--features hardware`; \
         use --loopback for the hardware-free self-test"
            .into(),
    )
}

fn poll_one(phy: &mut SondePhy, timeout: Duration) -> Option<sonde_phy::phy_api::RxFrame> {
    let start = Instant::now();
    loop {
        if let Some(f) = phy.poll_rx() {
            return Some(f);
        }
        if start.elapsed() > timeout {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
