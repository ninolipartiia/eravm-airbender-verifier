# Tier 3-lite — native whole-VM execution of the 1-tx attack (planned)

## Goal
The closest node-free approximation to an end-to-end test: run the **literal single
transaction** (a driver contract that far-call-loops across 33 max-size contracts) through
the real native `FastVmInstance` — the same VM the guest runs — and count the
`World::decommit` → `Program::new` calls it forces. This adds the far-call /
`pay_for_decommit` / callee-`ret` overhead that Tier 2 omits, and exercises #92's real
`ProgramCache` (not a copy) inside a real bootloader-driven batch.

## Why not a full `AirbenderVerifierInput` through `cycle_bench`
A runnable witness needs a genesis + the exact read-set (`StorageSnapshot` panics on any
missing slot, `snapshot.rs:50`) + consistent Merkle paths + commitment — the sequencer's
witness producer, absent from this reduced repo. No in-repo synthesizer; PR#97 tooling
only runs/transcodes. A true fixture ⇒ the Era-node route (`../../artifacts/decode-bench/
ATTACK-BATCH-SPEC.md`).

## Method (node-free)
1. **Bootstrap the genesis** from the real `84730` batch: load its `AirbenderVerifierInput`
   (`load_batch`), build an in-memory storage from its `witness_block_state` — this
   already contains the bootloader + system contracts + their bytecodes, a real genesis.
2. Wrap it in a **non-panicking fallback storage** (must be written; the only in-repo impl
   is the panicking `StorageSnapshot`). Inject: 33 max-size repetitive contracts
   (AccountCodeStorage + KnownCodesStorage entries + `used_bytecodes`), a driver contract,
   and fund an attacker account.
3. Build the driver tx (far-call loop over the 33) and run it through native
   `FastVmInstance` with `World::decommit` instrumented to count decodes (and log the
   `ProgramCache` hit/miss/evict tallies).
4. **Control:** run the identical setup on the unbounded (main) cache → expect 0
   re-decodes, proving #92 is the cause.

## Expectations (pre-registered — see EXPECTATIONS.md)
- ~1 decode per far call post-fill (100% miss) on the #92 build; **0** on main.
- Total decodes ≈ Tier 1's count, nearer the ~285k end (real per-iter ergs > 183).
- Cross-check: decodes × Tier 2's 47.4M cyc ≫ 2³⁶.

## Blockers / risk
- Writing the fallback storage wrapper + wiring `FastVmInstance` outside the verifier's
  normal entry point (the repo has no VM-execution harness — see `fail_closed.rs`, which
  only tampers existing batches and asserts rejection).
- If genesis bootstrapping is infeasible in reasonable effort, **descope**: the
  Tier1×Tier2 product already establishes the DoS quantitatively.

## Status — DONE (PASS), with the documented descope

Implemented as an in-crate test (`crates/multivm/src/versions/vm_fast/dos_tier3_test.rs`)
that drives the **real** `ProgramCache` + **real** `World::decommit`. Result
(`../results/tier3lite/`): BOUNDED (64 MiB, #92) → **99/99** thrash re-decodes (100% miss);
UNBOUNDED (`usize::MAX`, main) → **0/99**; real `World::decommit` = 6.39 ms/call. The
bounded-vs-unbounded control is the **causation proof** Tiers 1–2 lacked.

**Descope taken (as pre-registered):** the full bootloader-driven `FastVmInstance` tx run
was *not* built — it needs a synthesized valid tx + a complete genesis/read-set
(`StorageSnapshot` panics on any missing slot; no in-repo witness producer), i.e. the
node-gated blocker. The `World::decommit` call this test drives is exactly what vm2 invokes
per far call via `pay_for_decommit`; the far-call opcode's own erg cost (which bounds the
miss *count*) is accounted analytically in Tier 1. This is the closest node-free
approximation and it establishes causation on the real code.
