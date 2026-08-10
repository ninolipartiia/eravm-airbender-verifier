# Tier 3-lite results — real `ProgramCache` / `World::decommit` + causation control

Test: `crates/multivm/src/versions/vm_fast/dos_tier3_test.rs`
(`#[cfg(test)] mod dos_tier3_test`), run on branch `test/pr92-cycle-dos` (#92 present).
Raw output: `run.log`.

```
cargo test -p zksync_multivm --release dos_tier3 -- --ignored --nocapture
```

## Outcome: **PASS**

### Part B — real `ProgramCache`, bounded (#92) vs unbounded (main)
`World::decommit` runs `Program::new` **iff** `program_cache.get()` is `None`, so the
ProgramCache **miss count == the re-decode count, exactly**.

| cache | fill misses | thrash misses | thrash miss rate |
| --- | --- | --- | --- |
| **BOUNDED 64 MiB (#92)** | 33 / 33 | **99 / 99** | **100%** |
| UNBOUNDED `usize::MAX` (main) | 33 / 33 | **0 / 99** | 0% |

### Part A — real `World::decommit` (full path incl. `bytecode_cache`)
99 thrash far-calls at **6.39 ms/call**. A cache hit is ~µs; ms-scale per call proves the
real far-call path re-decodes (consistent with Tier 1's 7.47 ms and Tier 2's rv32 cost).

## Expectation vs result (pre-registered)
| prediction | result |
| --- | --- |
| ~1 decode per far call post-fill on #92 (100% miss) | 99/99 ✓ |
| **0** re-decodes on the unbounded cache | 0/99 ✓ |
| real `World::decommit` re-decodes (ms-scale/call) | 6.39 ms/call ✓ |

All three held. **This is the causation proof Tiers 1–2 lacked**: the *same* workload on
the *same* real code re-decodes 99× under #92's bounded cache and **0×** on main's
unbounded cache — so #92 is unambiguously the cause of the re-decode DoS.

## What this adds over Tiers 1–2
- Removes Tier 1's only caveat: it drives the **real** `ProgramCache` (not a verbatim copy)
  and the **real** `World::decommit` (the exact function vm2 calls via `pay_for_decommit`),
  including the real `bytecode_cache` path.
- Adds the **bounded-vs-unbounded control** → causally attributes the DoS to #92.

## Scope / honesty
- Built on the **#92-only** cherry-pick (isolates #92 as the cause). #93 would add a
  per-miss `load_factory_dep` clone (Tier 2 measured +2.6%); #94 is off this path.
- The `World::decommit` call is what vm2 invokes on every far call; this test drives it
  directly rather than through the far-call **opcode**. The opcode's own erg cost (183) is
  what bounds the miss *count* and is accounted analytically in Tier 1.
- **Not** a full bootloader-driven `FastVmInstance` tx run. That needs a synthesized valid
  transaction + a complete genesis/read-set, and `StorageSnapshot` panics on any missing
  slot (`snapshot.rs:50`) — there is no in-repo witness producer, so a runnable
  `AirbenderVerifierInput` fixture remains **node-gated** (see `../../tiers/TIER3-LITE.md`
  and `../tier2/data/ATTACK-BATCH-SPEC.md`). This was the pre-registered descope condition.

## Combined picture (all three tiers)
- Tier 1 (economics): 100% miss, **285k–390k** forced re-decodes per 80M-gas tx.
- Tier 2 (guest cycles): **47.4M** cyc per 2 MiB re-decode.
- Tier 3-lite (causation): the re-decodes are real and happen **only** under #92.
- ⇒ **197×–270× over 2³⁶**; break-even ~1,449 misses (~11% of one tx). DoS confirmed.
