# Independent validation review — PR#92 cycle-DoS, all three tiers

**Date:** 2026-08-08. **Branch:** `test/pr92-cycle-dos` @ `52d75b2`. **Reviewer stance:**
adversarial — re-derive every load-bearing constant from the *pinned* source, re-run what
is cheap, and hunt for anything that could make the results wrong or misleading.

## Verdict

**The central claim is correct and robust — and, where it errs, it errs conservatively
(toward *understating* severity).** A single ≤80M-gas transaction forces ~285k–390k full
`Program::new` re-decodes, each tens of millions of guest cycles, exceeding the 2³⁶
per-proof ceiling by ~**190×–270×**; break-even is only **~1,450 misses (~11% of one tx)**.
No defect was found that would flip the conclusion. Several minor consistency/clarity items
are listed below; two are worth a one-line doc fix.

## Verification ledger (independently re-derived / re-run)

| item | method | result |
| --- | --- | --- |
| far-call price = **183 ergs** | read `zkevm_opcode_defs` @ the **exact pinned rev `b60b7bd` (v0.153.13)** — not my stale local checkout | `2·4+1+150+20+2+1+1 = 183` ✓ |
| decommit = **4 ergs/word** | same pinned rev `circuit_prices.rs` | `CODE_DECOMMITMENT_COST_PER_WORD_IN_ERGS = 4` ✓ |
| vm2 far-call path uses that version | `Cargo.lock`: vm2 v0.6.0 → `zkevm_opcode_defs 0.153.13` | ✓ (correct version on the path) |
| max bytecode = **65,535 words (odd)** | `crates/basic_types/src/bytecode.rs` | `(1<<16)-1`, must be odd ✓ |
| Tier-1 `ProgramCache` copy is faithful | diffed against real `program_cache.rs` @ `1dc7ce8` | identical get/insert/evict/`program_bytes` ✓ |
| Tier-2 guest reproduces | live re-run k=64 → `2993048709` (bit-identical); full sweep byte-identical | ✓ |
| Tier-2 slope | `(8,913,601,413−2,993,048,709)/128` | **46,254,318** cyc/decode ✓ |
| Tier-3-lite | `cargo test … dos_tier3 --ignored` | BOUNDED 99/99, UNBOUNDED 0/99, 6.39 ms/call ✓ |
| `effective_cycles` formula | `artifacts/baseline/compare_runs.py` | `raw + 16·Blake2 + 4·keccak + 4·bigint` |

## Findings

### F1 — [Minor, fix recommended] Headline per-decode (47.43M) is the #92+#93 figure, but the branch is #92-only (→ 46.25M)
`tier1-sim/src/main.rs:128` hardcodes `TIER2_CYC_PER_DECODE = 47_434_425` (2 MiB **+#93
clone**). But this branch cherry-picked **#92 only**, and I confirmed `world.rs:179` still
runs `bytecode_cache.insert(hash, code)` in the storage fallback — so on a re-decode the
bytecode is served from `bytecode_cache` with **no `load_factory_dep` clone**. The correct
per-decode for the tested branch is the **no-clone 46,254,318** (Tier 2 measured both).
- **Impact:** ~2.6% **overstatement** of severity (the *unsafe* direction, but negligible).
  Corrected #92-only headline: **192×–262× over 2³⁶**, break-even **~1,486 misses**
  (vs 190×–270× / ~1,449). Conclusion unchanged.
- **Fix:** headline the 46.25M (#92-only) figure, or state explicitly that 47.43M models
  the intended **#92+#93+#94 stack** (both are legitimately measured; just label which).

### F2 — [Cosmetic] Cross-tier fill-cost mismatch
Tier 1 uses fill = `33·(65,535·4 + 183) = 8,656,659` ergs; Tier 2's `analyze.py` uses
`64 MiB / 8 = 8,388,608`. The ~0.27M-erg difference shifts the miss count by ~0.4%
(389,854 vs 391,319 @183). Both are defensible approximations; not misleading. Optionally
unify on the Tier-1 (more precise) value.

### F3 — [Not an error / conservative] raw `cycles_executed` vs `effective_cycles`
The 2³⁶ ceiling is on `effective_cycles = raw + Σ(weighted delegations)`, weights ≥ 0, so
**effective ≥ raw always** → comparing raw `cycles_executed` to 2³⁶ can only *understate*.
Moreover `Program::new` invokes **no** Blake2/keccak/bigint precompile (it is byte-parsing
+ opcode-table decode; `U256::from_big_endian` is a byte copy, not bigint arithmetic), so
delegations = 0 → **effective = raw**. Could not print delegations directly (the
decode-bench guest isn't built with `cycle-markers`), but the ≥ argument makes this airtight
regardless. **No action.**

### F4 — [Not an error / conservative] Tier 2 measures decode in isolation
The bench calls `Program::new` in a loop with a `BenchWorld` stub (a different `W` than the
real `World<S,T>`). Decode work is independent of `W` (same `instructions` + `code_page`
passes), so the cost is representative; and it **omits** the surrounding far-call /
`pay_for_decommit` / callee-`ret` cost the real attack also pays → Tier 2 is a **lower
bound** on per-miss cost. Tier 3-lite Part A corroborates via the real `World::decommit`
(6.39 ms/call, consistent). **No action.**

### F5 — [Verified sound] Tier 2 measures the *full* decode, not a hoisted/partial one
Checked the guest: bytecode is built once outside the timed loop; each iteration does a
fresh `Program::new` whose output is consumed via `black_box(code_page())` (blocks dead-code
elimination); `program` is dropped each iteration (no heap accumulation). The magnitude and
scaling prove the dominant `instructions` decode runs: 512 KiB=30.5M and 2 MiB=46.25M share
the capped 65,536-instruction decode (`D≈25M`) and differ only by the linear `code_page`
term (`≈320 cyc/word`); a `code_page`-only cost could not reach 46M. Linear in K
(64→2.99e9, 192→8.91e9, ratio 2.98≈3) ⇒ no hoisting. **Sound.**

### F6 — [Verified sound] Tier 3-lite causation control is valid
Part B drives the **real** `ProgramCache`; `World::decommit` runs `Program::new` **iff**
`program_cache.get()==None`, so miss count == re-decode count *by construction*. BOUNDED
(64 MiB) → 99/99 re-decodes; UNBOUNDED (`usize::MAX`, models main's non-evicting cache) →
0/99. Part A drives the **real** `World::decommit` end-to-end (6.39 ms/call ⇒ real decode,
vs µs for a hit). This is the causal attribution to #92 that Tiers 1–2 lack. **Sound.**

### F7 — [Scope caveat, already disclosed] Not full bootloader execution; attack prerequisites
- Tier 3-lite is **not** a full `FastVmInstance`/bootloader tx run — that needs a synthesized
  valid tx + complete genesis/read-set (`StorageSnapshot` panics on any missing slot); no
  in-repo witness producer, so a runnable `AirbenderVerifierInput` fixture is **node-gated**.
  Disclosed in `tiers/TIER3-LITE.md` and `results/tier2/data/ATTACK-BATCH-SPEC.md`.
- The "single tx" is the *attack* tx; it presupposes **≥64 MiB of distinct large bytecode
  is reachable on-chain** (either already deployed — mainnet holds far more than 64 MiB in
  aggregate — or deployed by the attacker in prior batches; repetitive bytecode is
  compression-cheap to publish). This setup does not change the single-tx-unprovable
  conclusion but is a real precondition worth stating in the README.

### F8 — [Robustness] Sensitivity of the conclusion
- Per-iteration cost: 183 (bare) is optimistic; realistic driver-loop overhead ⇒ ~223–250
  ergs ⇒ ~285k–320k misses. The docs already present the 183/223/250 range — not misleading.
- The conclusion is robust to large errors in the inputs: even at 250 ergs/iter **and** the
  no-clone 46.25M, the attack is ~192× over 2³⁶; break-even (~1,486 misses) is ~11% of one
  tx. It would take a >100× combined error to approach the ceiling — far outside any
  measured uncertainty.
- No per-tx far-call/decommit *count* limit exists in EraVM (only gas); `decommitted_hashes`
  stays at 33 entries; sequential far-calls don't accumulate stack/heap (callee `ret`s,
  `should_materialize=false` on repeat). So 390k far-calls in one tx is gas-limited only.

## Per-tier assessment
- **Tier 1 (economics):** logic sound; 100% miss is the correct adversarial worst case
  (in-order cycle over a working set > cap); arithmetic verified. One label fix (F1).
- **Tier 2 (guest cycles):** method sound (slope cancels setup), anti-hoisting effective,
  reproduced bit-identically, delegations provably 0. Strongest tier.
- **Tier 3-lite (causation):** sound; real code; valid control. Honest about its descope.

## Recommended actions
1. **F1:** relabel the headline per-decode as 46.25M (#92-only, matching the tested branch),
   keeping 47.43M as the "#92+#93 stack" figure. Re-state the multiple as ~190×–270× with
   the version each figure assumes.
2. **F2 (optional):** unify the fill-cost constant across Tier 1 and `analyze.py`.
3. Nothing else blocks relying on the result. The DoS is confirmed with wide margin.
