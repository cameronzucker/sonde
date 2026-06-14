# sonde-wasm

Real Sonde DSP over a simulated HF channel, exported to JS via wasm-bindgen.

## Host tests
```
cargo test -p sonde-wasm
```

## Build for the browser
```
cargo install wasm-bindgen-cli   # once
cargo build -p sonde-wasm --release --target wasm32-unknown-unknown
wasm-bindgen target/wasm32-unknown-unknown/release/sonde_wasm.wasm \
  --out-dir ../../demo/site/pkg --target web
```

## JS API (all return JSON strings unless noted)
- `list_modes() -> ModeInfo[]`
- `recommend_mode(snr_db: number) -> string` (mode id; plain string, not JSON)
- `run_link(payload: Uint8Array, offsets_json: string, mode_id: string, snr_db: number, condition: string, seed: number) -> LinkResult | {error}`
