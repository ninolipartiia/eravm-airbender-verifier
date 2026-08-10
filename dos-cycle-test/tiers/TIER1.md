# Tier 1 — native FIFO economics simulation

## Goal
Independently confirm the **count** half of the DoS: that #92's 64 MiB FIFO cache, driven
by the attack access pattern, yields a **100% miss rate**, and that an 80M-gas tx forces
**~285k–390k** full `Program::new` re-decodes. (The per-decode *cost* is Tier 2.)

## Method
Standalone Rust crate `dos-cycle-test/tier1-sim/` (native, `--release`) that:
1. Embeds the `ProgramCache` FIFO logic **verbatim** from
   `crates/multivm/src/versions/vm_fast/program_cache.rs` @ `1dc7ce8` (the cherry-picked
   #92). The copy is trivial (~30 lines: `new`/`get`/`insert`/`evict_to_cap` +
   `program_bytes = code_page().len()*32`); fidelity rests on it being identical, cited by
   commit. The expensive/subtle parts use the **real library**: `zksync_vm2::Program::new`
   for decode, and the real `code_page().len()` for eviction accounting.
2. Builds 33 distinct max-size (65,535-word) programs via `Program::new` (repetitive,
   deterministic bytecode — decode is content-independent).
3. Replays `World::decommit`'s exact per-far-call logic — `get(hash)`; on `None`,
   `Program::new` then `insert(hash, program)` (which evicts) — over the cyclic working
   set for a modest number of cycles (enough to prove steady state; NOT 390k, that would
   be ~1 h of real decodes).
4. Counts hits / misses / evictions / decodes; asserts 0 post-fill hits; times per-decode
   and per-far-call.
5. Computes the budget-limited miss count from the erg model and prints the 2³⁶ multiple
   using Tier 2's 47.4M cyc/decode.

## Fidelity notes / limitations
- The FIFO is a verbatim copy, not the linked symbol (`ProgramCache` is `pub(super)`).
  An optional in-crate `#[cfg(test)]` using the real type is a possible upgrade.
- Measures native x86 time, not guest cycles (that is Tier 2's job).
- Omits far-call/`pay_for_decommit`/callee overhead (that is Tier 3-lite).

## Pass/fail (see EXPECTATIONS.md)
PASS if: 100% post-fill miss, steady state = 32 entries, 1 eviction/access, and the
budget model reproduces ~285k–390k decodes. FAIL (revisit the model) if the miss rate is
materially below 100%.

## Reproduce
```
cd dos-cycle-test/tier1-sim && cargo run --release | tee ../results/tier1/run.log
```
