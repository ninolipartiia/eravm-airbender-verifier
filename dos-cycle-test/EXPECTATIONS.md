# Pre-registered expectations

Predictions recorded **before** each tier is run, so results can be compared and the plan
adjusted. Derived from the source-verified constants in `README.md`.

## Shared erg model (the attack economics)

- Optimal attack contract = **max size, 2,097,120 B = 65,535 words (odd)**. Only the
  instruction *decode* saturates at 512 KiB (`.take(1<<16)`); `code_page` + word passes
  scale to 2 MiB, so bigger = more work per equally-priced far call.
- Working set = **33 × 2 MiB ≈ 69.15 MiB > 64 MiB cap** → 100% FIFO miss (cache holds 32).
- Fill phase (33 first far-calls): decommit `65,535 words × 4 = 262,140` ergs each + 183
  → 33 × 262,323 ≈ **8.66M ergs**, does 33 decodes.
- Thrash budget: `80M − 8.66M ≈ 71.3M ergs`. Per repeat far call = **183 ergs**
  (decommit 0), plus a few ergs of caller-loop overhead.

| per-iter ergs | predicted misses |
| --- | --- |
| 183 (bare far-call) | ~390,000 |
| 223 (+loop) | ~320,000 |
| 250 (+loop) | ~285,000 |

**Predicted forced re-decodes under one 80M tx: ~285k–390k** (plus 33 fill decodes).

## Tier 1 — native FIFO economics (to run now)

Pre-registered pass criteria:
1. **100% miss** in the thrash phase for the 33×2 MiB cyclic pattern (structural: after
   the fill inserts the 33rd, the 1st is evicted; each subsequent cyclic access hits an
   evicted slot). Expect **0 hits** post-fill.
2. Cache steady state holds exactly **32** programs; **1 eviction per post-fill access**.
3. Forced-decode count under the 80M erg model reproduces **~285k–390k** (analytic, from
   the erg budget; the sim asserts the miss-rate and computes the count — it does NOT run
   390k real 11 ms decodes).
4. Native per-decode ≈ **~11 ms** for 2 MiB (x86, matches `scratchpad/decodebench`), and
   the decode dominates per-far-call cost (cache bookkeeping is negligible).

Prediction for the headline: Tier1 count (~285k–390k) × Tier2 cyc/decode (47.4M) ⇒ well
over 2³⁶. If the miss-rate is **not** 100%, the plan must be revisited (the FIFO/erg model
is wrong).

## Tier 2 — real guest cycles per decode  ⚠️ ALREADY RUN (this session)

**Pre-registration (original theory, from the first written analysis):** ~1.4M guest
cyc/load ⇒ attack ≈ **8× 2³⁶**. (That 1.4M was a figure for tiny ≤24 KiB EVM contracts.)

**Measured result (`../artifacts/decode-bench/RESULTS.md`):** 2 MiB `Program::new` =
**46.25M** guest cyc (+#93 clone = 47.43M, +2.6%); 512 KiB = 30.5M; ratio 2 MiB/512 KiB =
1.52× (confirms no saturation at 512 KiB). ⇒ attack = **198×–270× over 2³⁶**;
break-even at only **~1,449 misses (~11% of one 80M tx)**.

**Lesson already applied:** theory understated severity **~34×**; mechanism/direction were
correct. This is exactly the "compare before proceeding" adjustment — Tier 1's job is now
to independently confirm the *count* half (the miss-rate + budget), and Tier 3-lite to add
the far-call/bootloader overhead the isolated decode omits.

## Tier 3-lite — native whole-VM execution (planned)

Pre-registered expectations:
1. A single far-call-loop tx, run natively through `FastVmInstance` against a storage
   bootstrapped from the real `84730` snapshot + injected 33×2 MiB contracts, drives
   `World::decommit` → `Program::new` on **every** far call, with an instrumented counter
   showing **~1 decode per far call** post-fill (100% miss under #92's bounded cache).
2. Decode count over the tx ≈ the Tier 1 figure (~285k–390k), now including real far-call/
   `pay_for_decommit`/callee-`ret` overhead → **fewer** iterations per erg than the bare
   183 model, i.e. count nearer the ~285k end.
3. On the **unbounded** (main) cache, the same run does **0** re-decodes (each hash decoded
   once) — the control that proves #92 is the cause.

Risk/blockers: needs a non-panicking fallback storage wrapper (none in-repo) + the 84730
genesis reuse; commitment/Merkle phases are **not** required (cycles are burned in
execution). If bootstrapping the genesis proves infeasible, Tier 3-lite is descoped and
the Tier1×Tier2 product stands as the result.
