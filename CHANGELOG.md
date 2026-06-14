# Changelog

## 1.0.0 (2026-06-14)


### Features

* **analysis:** per-sub-carrier SNR estimator + serde output ([711950c](https://github.com/cameronzucker/sonde/commit/711950c4c50d5276683d813551a404024b0ae451))
* **channel:** two-tap Watterson WattersonChannel core ([e2fe2bc](https://github.com/cameronzucker/sonde/commit/e2fe2bc4c33bd62dc2faffee25741e2dae74acbd))
* **cli:** pipe-friendly hf-channel-sim-cli for AI-agent harnesses ([7f0350a](https://github.com/cameronzucker/sonde/commit/7f0350a7a66955b87778ab28157771a70af19ddd))
* **fading:** spectrum-shaped complex-Gaussian Watterson tap process ([93528b7](https://github.com/cameronzucker/sonde/commit/93528b74dedccce7f88aafe55a5a37093e8ebd54))
* **hf-channel-sim:** initial AGPLv3 crate scaffolding ([ca936b3](https://github.com/cameronzucker/sonde/commit/ca936b3876aa138dbb8593a118a517a9a8334a12))
* **noise:** AWGN generator decoupled from channel ([3755ed0](https://github.com/cameronzucker/sonde/commit/3755ed0aed16b5ceff50cfaf0ae8acf7e044c6f1))
* **params:** ITU-R F.520 + F.1487 channel condition vocabulary ([3c28f7d](https://github.com/cameronzucker/sonde/commit/3c28f7d574f69f9f74dba4d3944cf98247ed8914))
* **report:** end-to-end characterization report + JSON ([2945beb](https://github.com/cameronzucker/sonde/commit/2945beb6c35c70a8831c1d60aba714b6e557a7a6))
* **rng:** seeded Xoshiro256++ + complex Gaussian draws ([44865bc](https://github.com/cameronzucker/sonde/commit/44865bcb671a2e4a0778ca827ffbb599e3f8aa08))
* **sonde-fec:** FloorRate14Codec — rate-1/4 LDPC over the FecCodec bus ([aeb745d](https://github.com/cameronzucker/sonde/commit/aeb745d6b2a7f6cf6c715b04584a089e7a0b5357))
* **sonde-fec:** IRA dual-diagonal rate-1/4 floor code (encodable by construction) ([4093fd1](https://github.com/cameronzucker/sonde/commit/4093fd197842ca4546d23370b91269de55235320))
* **sonde-phy-runtime:** core PhyTransport runtime over Waveform + Radio seams (sonde-gmc) ([84424de](https://github.com/cameronzucker/sonde/commit/84424de210bd00095ff1109d0606a7d15786f347))
* **sonde-phy-runtime:** production SoundcardRadio behind `hardware` feature (sonde-gmc) ([510d392](https://github.com/cameronzucker/sonde/commit/510d392e21d20c8ee96792fef1f94acdb20fa29b))
* **sonde-phy:** codeword-spanning coded-floor framing (coded_framing) ([b70ad78](https://github.com/cameronzucker/sonde/commit/b70ad787c040bcbe70edd4c57548a9c77a76584a))
* **sonde-phy:** floor runs one coded codeword-per-block soft-LLR path ([46a4586](https://github.com/cameronzucker/sonde/commit/46a4586ff3cf964c045e3b165a5d998efb9fc93a))
* **sonde-rx:** inject FloorRate14Codec into decode_one_symbol (real FEC on RX) ([f82c5c5](https://github.com/cameronzucker/sonde/commit/f82c5c515ab6dc7edf8dfcdacfe45dfae2bbf1ff))
* **sonde-tx:** inject FloorRate14Codec into encode_payload (real FEC on TX) ([e3078aa](https://github.com/cameronzucker/sonde/commit/e3078aab37cf969d6539aa4f85743287a0119cee))
* **tux-rig-cm108:** CM108-HID PTT primitive + CLI (tuxlink-u1js) ([fb01289](https://github.com/cameronzucker/sonde/commit/fb01289d79d35d4103ec8b209c17e4830ae62c94))
* **tux-rig-rts:** serial-RTS PTT primitive + CLI (tuxlink-mxyz) ([5073ccf](https://github.com/cameronzucker/sonde/commit/5073ccf02da0df28a65a054cf1cad7c30949d923))
* **tux-rig-rts:** tux-rig-watchdog SIGKILL-safe PTT daemon (tuxlink-23ps, Phase 1.5) ([a383aaa](https://github.com/cameronzucker/sonde/commit/a383aaa704a4d3e3f2d93a62362313ab8fd71dd6))
* **tux-rig-watchdog:** PR_SET_PDEATHSIG belt-and-suspenders parent-death detection (tuxlink-a2z0) ([5d6743a](https://github.com/cameronzucker/sonde/commit/5d6743a71b457b4fdb85c2321b8f44467565c599))
* **tuxmodem-fec:** block bit interleaver with burst-decorrelation gate ([a263b00](https://github.com/cameronzucker/sonde/commit/a263b006c5b375a839e6889b7fe8438759e7344c))
* **tuxmodem-fec:** CRC-32 append + verify over bit slices ([cdf7570](https://github.com/cameronzucker/sonde/commit/cdf7570490b7a2f6fe7c0d07a2deec32f4e13d6f))
* **tuxmodem-fec:** FecCodec impl wiring CRC + LDPC + interleaver ([6dc09ef](https://github.com/cameronzucker/sonde/commit/6dc09eff8402a7b6dc6ac3dda717af1c3b5327d5))
* **tuxmodem-fec:** LDPC systematic encoder + WiFi-family seed iteration ([131e484](https://github.com/cameronzucker/sonde/commit/131e484ad0854615b32adce8cf2734c711f79b73))
* **tuxmodem-fec:** parity-check matrix + floor rate-1/4 + WiFi family LDPC codes ([792e415](https://github.com/cameronzucker/sonde/commit/792e41567f77cba5112a88ee0e3469e4e44b2780))
* **tuxmodem-fec:** scaffold AGPLv3 crate for clean-sheet LDPC FEC ([d14291d](https://github.com/cameronzucker/sonde/commit/d14291dc1705259c02452fe72418a24708a994d8))
* **tuxmodem-fec:** SPA belief-propagation decoder (LLR-form) ([c187730](https://github.com/cameronzucker/sonde/commit/c18773044e396151eb6e6b5b19e02f96f04915ec))
* **tuxmodem-phy:** 48kHz f32 audio buffer + wav round-trip helper ([7661c2d](https://github.com/cameronzucker/sonde/commit/7661c2de629aa531d21e6ed0c91038c5b67e859c))
* **tuxmodem-phy:** audio_device module + tuxmodem-audio-play bench CLI (tuxlink-h8pp) ([f0cbddc](https://github.com/cameronzucker/sonde/commit/f0cbddc5f255cd3a2959b775ad72ab01d349c678))
* **tuxmodem-phy:** BPSK / QPSK / 16-QAM / 64-QAM + max-log LLR ([0ab1b09](https://github.com/cameronzucker/sonde/commit/0ab1b09d5640e64f190706f04c4b24337dd50308))
* **tuxmodem-phy:** channel-sim adapter + BER sweep + ARDOP competence gate ([b33a680](https://github.com/cameronzucker/sonde/commit/b33a68008088795a34ee4a8b14b3d21b262a6cd3))
* **tuxmodem-phy:** crate skeleton + error taxonomy ([0eba17b](https://github.com/cameronzucker/sonde/commit/0eba17b5230507cb96d412d71fe2fc03b7cf0974))
* **tuxmodem-phy:** FEC bus contract + SNR-aware mode router + FT-818 gate ([13940c6](https://github.com/cameronzucker/sonde/commit/13940c6033286e3337e918b811a5d7e20fc6672b))
* **tuxmodem-phy:** mode table + ModeHint/ResolvedMode/ModeFamily skeleton ([9157f6e](https://github.com/cameronzucker/sonde/commit/9157f6ec618ffe7a0409434fb63e853d0014fbd4))
* **tuxmodem-phy:** multi-symbol + preamble composition (tuxlink-k2xv, Phase 10 slice 2) ([612f6e4](https://github.com/cameronzucker/sonde/commit/612f6e42edbdde7c822c73fb6118e312b08fb6f4))
* **tuxmodem-phy:** multi-symbol framing primitive (tuxlink-cwjp, Phase 10 slice 1) ([a1dadbd](https://github.com/cameronzucker/sonde/commit/a1dadbd097507d019c61423f9490631703c59e02))
* **tuxmodem-phy:** narrow-FSK situational floor mode ([03ccccc](https://github.com/cameronzucker/sonde/commit/03ccccc3ad70ced7e57f2da9945301fd6ecb99a8))
* **tuxmodem-phy:** OFDM equalizer + receiver (clean-channel round-trip) ([0757413](https://github.com/cameronzucker/sonde/commit/07574137fffe6c7e9aa824fed5b9e28c90ab9e63))
* **tuxmodem-phy:** OFDM mode parameter table (Narrow/Mid/Wide) ([a771281](https://github.com/cameronzucker/sonde/commit/a7712811bacb7e162f5494f0787a0722049d5ba5))
* **tuxmodem-phy:** OFDM transmitter (one-symbol modulate) ([66f2e70](https://github.com/cameronzucker/sonde/commit/66f2e70aca9891a02f0a2b2da2074ffd88f05939))
* **tuxmodem-phy:** PhyTransport API + NullPhy contract baseline ([60e282a](https://github.com/cameronzucker/sonde/commit/60e282a5c5d444599ef910720b0279eee6a731ce))
* **tuxmodem-phy:** pilot-aided per-subcarrier SNR estimator (Phase 5) ([e5440f1](https://github.com/cameronzucker/sonde/commit/e5440f122f1427aaab1bc300ae583b8ff10a5a74))
* **tuxmodem-phy:** preamble round-trip primitive (tuxlink-iyl9, Phase 12 slice 1) ([b101105](https://github.com/cameronzucker/sonde/commit/b101105eb864fcc73afe5b24e8c4dbef2984481a))
* **tuxmodem-phy:** synchronization infrastructure (Phase 4) ([7984937](https://github.com/cameronzucker/sonde/commit/7984937991ff94ad127dd27af45db0c0e4958c1d))
* **tuxmodem-phy:** water-filling per-subcarrier bit-loader ([790d87a](https://github.com/cameronzucker/sonde/commit/790d87a238f448d7bc0df48243cc60a1d177193d))
* **tuxmodem-phy:** wide-band low-density OFDM floor (default robustness mode) ([efe9f1f](https://github.com/cameronzucker/sonde/commit/efe9f1f3bd71135b4e7e83830343eeaf9bb92a8d))
* **tuxmodem-rx:** capture + demod + BER CLI (tuxlink-xvrb) ([a1ce9a7](https://github.com/cameronzucker/sonde/commit/a1ce9a7094a40e5c2c339cd50e278ecde6a0b78f))
* **tuxmodem-tx:** --watchdog flag spawns tux-rig-watchdog for SIGKILL-safe TX (tuxlink-8xfa, Phase 1.5 slice 2) ([a8f1a36](https://github.com/cameronzucker/sonde/commit/a8f1a3630da57f3a7447188e67401eecece4efa2))
* **tuxmodem-tx:** --write-wav PATH (encode to file, no device/PTT) — tuxlink-4dv9 ([a1069d2](https://github.com/cameronzucker/sonde/commit/a1069d22a008692b759b1725afc77a55c82d0b7f))
* **tuxmodem-tx,tuxmodem-rx:** --frame-mode multi-sync (tuxlink-ot37, Phase 10 slice 3) ([a2428e8](https://github.com/cameronzucker/sonde/commit/a2428e83c271a46baa0da6a0863a6c1708a1e2fb))
* **tuxmodem-tx,tuxmodem-rx:** --frame-mode raw|sync CLI wiring (tuxlink-fxmc, Phase 12 slice 2) ([95041d1](https://github.com/cameronzucker/sonde/commit/95041d1270e0acc52e82cc28d4c9ee1cd2e41440))
* **tuxmodem-tx:** payload → PHY → PTT + audio CLI (tuxlink-i3bz) ([df58a91](https://github.com/cameronzucker/sonde/commit/df58a91195d1b38542bb64d67aaf3d3fcb74c845))
* **tuxmodem:** scaffold AGPLv3 workspace for clean-sheet modem ([68a4dc9](https://github.com/cameronzucker/sonde/commit/68a4dc9556f67d54fe57ec3ea3f401e50a1529f1))


### Bug Fixes

* **governance:** resolve commit branch from cwd, not the main checkout ([5b726b5](https://github.com/cameronzucker/sonde/commit/5b726b5238d7fbeb78b8cf546b5614168ef56882))
* **sonde-phy:** bound floor codec box as Send for downstream worker-thread adapters ([86feb40](https://github.com/cameronzucker/sonde/commit/86feb40a5b3fd31bced3ef356c55680d0e34cda0))
* **sonde-phy:** drop duplicate truncation test name (B3 compile fix) ([d5054be](https://github.com/cameronzucker/sonde/commit/d5054be50feeb6a8681ef4ae5e3f8b6a504b6b02))
* **sonde-phy:** use array literal in transmit_multi test (clippy::useless_vec) ([14dedf1](https://github.com/cameronzucker/sonde/commit/14dedf13aaf35ef1c4e59f334f1e97d2b350c248))
* **sonde-tx:** use !is_empty() in encoder test assertion (clippy::len_zero) ([94209a8](https://github.com/cameronzucker/sonde/commit/94209a8a9e63b42b2696170efe8e0385050b8c00))


### Refactors

* **sonde:** rename to Sonde + vendor hf-channel-sim + CI ([a8bd6f7](https://github.com/cameronzucker/sonde/commit/a8bd6f7f6864db167c56bf54d0b37c939579b42e))

## Changelog

All notable, user-facing changes to Sonde are recorded here.

This file is maintained by [release-please](https://github.com/googleapis/release-please)
from [Conventional Commit](https://www.conventionalcommits.org) messages — do not
hand-edit released sections. The project-level version tracked here (and in
`version.txt`) is **decoupled from the per-crate `Cargo.toml` versions**; see
[VERSIONING.md](VERSIONING.md) and [ADR 0005](docs/adr/0005-semver-via-release-please.md).

<!-- release-please will insert released sections below this line. -->
