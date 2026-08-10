# PR#92 re-decode cycle-DoS — empirical validation

**Branch:** `test/pr92-cycle-dos` = PR#97 tooling (`602ba57`) + decode-bench harness
(`6df5f56`) + **PR#92** cherry-picked (`1dc7ce8`, `program_cache.rs` present).
**Date:** 2026-08-08. **Owner:** security review.

> **Independent review:** see [`INDEPENDENT-REVIEW.md`](INDEPENDENT-REVIEW.md). Verdict:
> the conclusion is correct and conservative. One label fix (F1): this branch is **#92-only**,
> where a re-decode is the **no-clone 46.25M** guest cyc (`bytecode_cache` serves repeats);
> the **47.43M** used below is the **#92+#93** figure (adds the `load_factory_dep` clone).
> Both are measured; the multiple is **~190×–270× over 2³⁶** either way.

## What we are testing

PR#92 replaces the fast VM's unbounded decoded-`Program` cache (`World::decommit`)
with a **64 MiB FIFO** (`ProgramCache`, `PROGRAM_CACHE_CAP_BYTES = 64 << 20`). vm2 calls
`world.decommit(hash)` on **every** far call, and a repeat far call to an
already-decommitted hash costs **0 decommit ergs** (`vm2 decommit.rs:124`) yet still runs
`world.decommit()` unconditionally (`:199`). So once the working set exceeds the cache,
FIFO evicts the very entry about to be requested → **100% miss → a full `Program::new`
re-decode per far call**, all for the flat far-call price. That host-side decode is
invisible to EraVM accounting (`CycleStats::Decommit` only fires when `is_new`), so no
in-VM ceiling can see it.

**Claim under test:** a single ≤80M-gas transaction can force enough re-decodes to blow
the airbender per-proof cycle budget **2³⁶ = 68,719,476,736**, making a valid batch
unprovable (a liveness DoS, since a priority tx cannot be skipped).

Key constants (all source-verified — see `INVESTIGATION` section):

| constant | value | source |
| --- | --- | --- |
| far-call base price | **183 ergs** | `far_call.rs::ergs_price` (2·4+1+150+20+2+1+1) |
| decommit cost (repeat) | **0** | `vm2 decommit.rs:124` `if was_decommitted {0}` |
| decode cap (instructions) | 65,536 | `vm2 program.rs:150` `.take(1<<16)` |
| max bytecode | **2,097,120 B** (65,535 words, odd) | `basic_types/bytecode.rs` |
| cache cap | 64 MiB bytecode | `PROGRAM_CACHE_CAP_BYTES` |
| tx compute budget | 80,000,000 ergs | `TX_MAX_COMPUTE_GAS_LIMIT` |
| per-proof cycle ceiling | 2³⁶ ≈ 6.87×10¹⁰ | airbender |

## Three-tier method (increasing fidelity, increasing cost)

| tier | what it measures | fidelity | needs a node? | status |
| --- | --- | --- | --- | --- |
| **Tier 1** | attack **economics**: 100%-miss FIFO behavior + forced-decode **count** under an 80M budget, against real `Program::new` + a verbatim `ProgramCache` | model of the cache path | no | **PASS** → `results/tier1/` |
| **Tier 2** | **real guest cycles** per `Program::new` (rv32, via the transpiler) — the per-miss cost | real guest cycles, isolated decode | no | **done** → `results/tier2/`, `../artifacts/decode-bench/` |
| **Tier 3-lite** | real `ProgramCache` + real `World::decommit`, with a **bounded-vs-unbounded causation control** (descoped from full bootloader run — node-gated) | real #92 code path | no | **PASS** → `results/tier3lite/` |

## Headline result (all three tiers agree)

A single ≤80M-gas transaction forces **~285k–390k** full `Program::new` re-decodes
(Tier 1), each **~47.4M** guest cycles (Tier 2) ⇒ **1.35–1.85×10¹³ cycles = 197×–269×
over the 2³⁶ ceiling.** Break-even is only **~1,449 misses (~11% of one tx's gas)**.
Tier 3-lite proves **causation on the real code**: the same workload re-decodes **99/99**
times under #92's bounded cache and **0/99** on main's unbounded cache. The batch is
unprovable; the DoS is confirmed with wide margin, and attributable to #92.

**Product that proves the DoS:** (Tier 1 forced-miss count) × (Tier 2 cyc/decode) vs 2³⁶.

Why not a full end-to-end `AirbenderVerifierInput` fixture through `cycle_bench`? Because
producing a *runnable* witness needs (a) a genesis, (b) the exact read-set (the
`StorageSnapshot` panics on any missing slot, `snapshot.rs:50`), and (c) consistent
Merkle paths + commitment — i.e. the sequencer's witness producer, which is **not in this
reduced repo**. PR#97 tooling only *runs*/transcodes batches; it has no generator. Tier
3-lite is the node-free approximation (bootstraps the genesis off `84730`).

## Layout

```
dos-cycle-test/
  README.md              ← this file (overall investigation + status)
  EXPECTATIONS.md        ← pre-registered quantitative predictions (compare against)
  tiers/TIER1.md         ← Tier 1 design, method, pass/fail criteria
  tiers/TIER2.md         ← Tier 2 design + result (consolidated from artifacts/decode-bench)
  tiers/TIER3-LITE.md    ← Tier 3-lite design + blockers
  tier1-sim/             ← Tier 1 harness (standalone Rust crate)
  results/tier1/         ← Tier 1 outputs + comparison-to-expectation
  results/tier2/         ← pointer/summary of artifacts/decode-bench
```

## Method discipline (per user request)

Each tier's **expectation is pre-registered** in `EXPECTATIONS.md` *before* running it, and
each tier's result is compared to that expectation **before** the next tier is
implemented — so a surprise (e.g. Tier 2 already showed theory understated severity ~34×)
can adjust the plan.
