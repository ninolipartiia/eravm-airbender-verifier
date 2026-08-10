# Tier 2 results — real guest cycles per `Program::new`

**Executed and re-verified live on branch `test/pr92-cycle-dos` (2026-08-08).** Full data
committed in-tree here (the source artifacts live under the repo's gitignored
`artifacts/decode-bench/`; copied into `data/` + `guest/` so the fork carries them).

## Reproduction check (this branch)
- Guest `app.bin` sha256 = `49f9facf3e6f5bac84c3b70d86d6aacb23d51b2e6692705465c8b0db0d1e58b9`
  (matches the recorded build; see `guest/app.bin.sha256`).
- Live single-point re-run: `k=64, 2 MiB, no-clone → cycles_executed=2993048709`, **exactly**
  the original (transpiler counts are deterministic).
- Full sweep re-run reproduced every row of `data/sweep.csv` byte-identically;
  `data/analyze-output.txt` is the fresh analysis.

## Measured guest cycles per `Program::new` (setup-subtracted; `data/sweep.csv`)
| bytecode | words | cyc/decode |
| --- | --- | --- |
| 96 KiB | 3,072 | 5,716,514 |
| 512 KiB | 16,384 | 30,476,841 |
| 1 MiB | 32,768 | 35,736,111 |
| **2 MiB (max)** | **65,535** | **46,254,318** |
| 2 MiB + #93 clone | 65,535 | 47,434,425 (+2.6%) |

- 2 MiB / 512 KiB = **1.52×** → no saturation at 512 KiB; max-size optimal.
- #93 per-miss `load_factory_dep` clone add-on @2 MiB = 1,180,107 cyc (+2.6%).

## Attack total vs 2³⁶ (`data/analyze-output.txt`)
| per-iter ergs | misses | total cycles | × 2³⁶ |
| --- | --- | --- | --- |
| 183 | 391,319 | 1.856×10¹³ | **270.1×** |
| 223 | 321,127 | 1.523×10¹³ | **221.7×** |
| 250 | 286,446 | 1.359×10¹³ | **197.7×** |

Break-even ~1,449 misses (~11% of one 80M tx).

## Expectation vs result
Pre-registered theory: ~1.4M cyc/load ⇒ ~8× 2³⁶. Measured: ~47M cyc/decode ⇒ ~200–270×.
**Theory understated severity ~34×** (the 1.4M was for tiny ≤24 KiB EVM contracts; max-size
EraVM decode is far heavier). Mechanism/direction confirmed.

## Files
- `data/sweep.csv`, `data/sweep.log`, `data/sweep-rerun.log` — raw transpiler cycle counts.
- `data/analyze.py`, `data/analyze-output.txt` — per-decode slopes + 2³⁶ table.
- `data/run_sweep.sh` — driver. `data/RESULTS.md` — original write-up.
- `data/gen_attack_contracts.py`, `data/ATTACK-BATCH-SPEC.md` — Tier-3/e2e attack recipe.
- `guest/app.bin` + `app.text` + `app.bin.sha256` — the exact sha-pinned decode-bench guest
  measured (calibration-only; production guest byte-identical, feature off by default).

## Caveat
Measures the isolated decode (+#93 clone) only — a **conservative lower bound** on real
per-miss cost (omits far-call/`pay_for_decommit`/callee-`ret`). Tier 3-lite adds that.

## Reproduce
```
cd guest && cargo airbender build --project . -- --features decode-bench   # → app.bin (sha above)
cargo build --release -p zksync_cycle_model --example decode_bench
bash dos-cycle-test/results/tier2/data/run_sweep.sh   # or artifacts/decode-bench/run_sweep.sh
python3 dos-cycle-test/results/tier2/data/analyze.py
```
