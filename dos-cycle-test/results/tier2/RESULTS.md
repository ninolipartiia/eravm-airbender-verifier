# Tier 2 results — real guest cycles per Program::new (pointer)

Full artifacts: `../../../artifacts/decode-bench/` (RESULTS.md, sweep.csv, analyze.py,
gen_attack_contracts.py, ATTACK-BATCH-SPEC.md). Guest committed as `6df5f56` on this branch.

Headline: 2 MiB `Program::new` = **46,254,318** guest cyc (+#93 clone = 47,434,425).
512 KiB = 30,476,841 → 2 MiB/512 KiB = 1.52x (no saturation at 512 KiB).
=> attack = **198x-270x over 2^36**; break-even ~1,449 misses (~11% of one 80M tx).

Expectation vs result: theory used ~1.4M cyc/load => ~8x; measured ~47M => ~200-270x.
Theory understated severity ~34x (the 1.4M was for tiny <=24 KiB EVM contracts).
Mechanism/direction correct.
