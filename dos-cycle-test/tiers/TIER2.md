# Tier 2 — real guest cycles per `Program::new`  (DONE)

## Goal
Measure the **real rv32 guest-cycle cost** of one `Program::new` decode — the per-miss
work #92 forces — instead of relying on the theory's ~1.4M/load figure.

## Method (as executed this session)
- A feature-gated `decode-bench` guest (`guest/src/main.rs`, `--features decode-bench`)
  calls the real `zksync_vm2::Program::new` in a timed loop of `K` decodes over a
  configurable bytecode size; optional per-iteration `Vec<u8>` clone models #93's
  `load_factory_dep` copy.
- Ground-truth `cycles_executed` comes from the PR#97 transpiler runner
  (`crates/cycle_model/examples/decode_bench.rs`).
- Per-decode = slope between two `K` values (setup cancels).
- Committed on this branch as `6df5f56`. Guest is **calibration-only** (feature off by
  default; production guest byte-identical).

## Result (`../../artifacts/decode-bench/RESULTS.md`, `sweep.csv`)

| bytecode | words | cyc/decode |
| --- | --- | --- |
| 96 KiB | 3,072 | 5,716,514 |
| 512 KiB | 16,384 | 30,476,841 |
| 1 MiB | 32,768 | 35,736,111 |
| **2 MiB (max)** | **65,535** | **46,254,318** |
| 2 MiB + #93 clone | 65,535 | 47,434,425 (+2.6%) |

- 2 MiB / 512 KiB = **1.52×** → cost does not saturate at 512 KiB; max-size optimal.
- Combined with the miss count: **198×–270× over 2³⁶**; break-even **~1,449 misses
  (~11% of one 80M tx)**.

## Expectation vs result
Pre-registered theory: ~1.4M cyc/load ⇒ ~8× 2³⁶. Measured: ~47M cyc/decode ⇒ ~200–270×.
**Theory understated severity ~34×** (the 1.4M figure was for tiny ≤24 KiB EVM contracts;
max-size EraVM decode is far heavier). Mechanism and direction confirmed.

## Caveat
Measures the isolated decode (+#93 clone) only — a **conservative lower bound** on real
per-miss cost, since it omits far-call/`pay_for_decommit`/callee-`ret` overhead. Tier
3-lite adds that.

## Reproduce
```
cd guest && cargo airbender build --project . -- --features decode-bench
cargo build --release -p zksync_cycle_model --example decode_bench
bash ../artifacts/decode-bench/run_sweep.sh && python3 ../artifacts/decode-bench/analyze.py
```
Guest app.bin sha256: `49f9facf3e6f5bac84c3b70d86d6aacb23d51b2e6692705465c8b0db0d1e58b9`
