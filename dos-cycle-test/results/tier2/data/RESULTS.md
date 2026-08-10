# PR#92 re-decode cycle DoS — measured vs theory (2026-08-08)

Goal: double-check the theoretical cycle-DoS calculation with a **real guest-cycle
measurement**. Method: a custom `decode-bench` guest (`guest/src/main.rs`, behind the
`decode-bench` feature) calls the real `zksync_vm2::Program::new` in a timed loop; the
transpiler runner (`crates/cycle_model/examples/decode_bench.rs`) reports ground-truth
`cycles_executed`. Per-decode = slope between two K values (setup cancels).

Build note: `Program::new` is identical across `main`/#92/#93/#94, so the per-decode
cost is branch-independent. #92 sets the *count* (evict→re-decode); #93 adds a per-miss
bytecode clone (measured below); #94 doesn't touch the far-call decommit path.

## Measured guest cycles per `Program::new` (setup-subtracted)

| bytecode | words | cyc/decode | notes |
| --- | --- | --- | --- |
| 96 KiB | 3,072 | 5,716,514 | |
| 512 KiB | 16,384 | 30,476,841 | decode cap (65,536 instr) reached here |
| 1 MiB | 32,768 | 35,736,111 | |
| **2 MiB (max)** | **65,535** | **46,254,318** | optimal attack contract |
| 2 MiB + #93 clone | 65,535 | 47,434,425 | #93 add-on = +1.18M (**+2.6%**) |

- **2 MiB / 512 KiB = 1.52× on rv32** (native x86 measured 1.83×). Confirms the
  correction that cost does NOT saturate at 512 KiB: only the instruction *decode* caps
  at `.take(1<<16)`; the `code_page` (`U256::from_big_endian` ×65,535) + `u64`-word
  passes scale to 2 MiB. **Max-size contracts are optimal**, not 512 KiB.
- **#94**: not on this path (touches `decommit_code`), no effect.

## Combined with the analytic miss-count → vs 2^36 (= 68,719,476,736)

Per far call (repeat) = 183 ergs, decommit cost 0; fill 64 MiB cache = 8.39M ergs;
thrash budget ≈ 71.6M ergs. Using measured **47.43M cyc/decode** (2 MiB +#93):

| per-iter ergs | misses | total cycles | × 2^36 |
| --- | --- | --- | --- |
| 183 | 391,319 | 1.86×10¹³ | **270×** |
| 223 | 321,127 | 1.52×10¹³ | **222×** |
| 250 | 286,446 | 1.36×10¹³ | **198×** |

- **Break-even: only ~1,449 misses reach 2^36** — costing fill + 1,449·183 ≈ **8.65M
  ergs, ~10.8% of one 80M-gas tx.** The full budget overshoots by ~200–270×.
- Even the unavoidable fill-phase (33 first-time decodes) is ~1.53×10⁹ cyc (~2.2% of 2^36).

## Verdict

**Theory confirmed in mechanism and direction; magnitude was UNDERSTATED ~34×.** The
review/first-answer used ~1.4M cyc/load (a figure measured on tiny ≤24 KiB EVM
contracts) → ~8× 2^36. The true per-decode for max-size EraVM contracts is **~47M guest
cycles**, so the attack is **~200–270× over the budget**, and blows it at ~11% of a
single tx's gas.

Caveats: (1) this measures only `Program::new` (+#93 clone) — it OMITS the far-call /
`pay_for_decommit` / callee-`ret` overhead the real attack also pays per miss, so it is a
**conservative lower bound** on real per-miss cost. (2) 100% FIFO miss for a >64 MiB
cyclic working set is structural, not measured here. (3) End-to-end confirmation through
a real `AirbenderVerifierInput` still needs the Era-node route (ATTACK-BATCH-SPEC.md) —
no in-repo witness synthesizer exists.

## Reproduce
```
cd guest && cargo airbender build --project . -- --features decode-bench
cargo build --release -p zksync_cycle_model --example decode_bench
bash artifacts/decode-bench/run_sweep.sh && python3 artifacts/decode-bench/analyze.py
```
Guest app.bin sha256: `49f9facf3e6f5bac84c3b70d86d6aacb23d51b2e6692705465c8b0db0d1e58b9`
