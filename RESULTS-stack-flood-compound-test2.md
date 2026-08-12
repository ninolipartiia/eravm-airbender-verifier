> **⚠️ SUPERSEDED IN PART — see "Revision 2" at the end.** A self-review found the phase-1 attack
> shape in the original run was suboptimal (it materialized stacks *before* recursing, which caps
> depth) and that the driver under-funded the flood phase by 1/64. Both are fixed. The corrected
> numbers are materially worse: **a single 20M tx now FAILS the 950 cap (963 MiB, −13)** where this
> document's Case A originally reported PASS +68. Case A/B tables below are the pre-fix measurements;
> the mechanism, arm-independence and methodology sections remain valid.

# Test 2 — stack pool + decommit flood, co-resident (1 tx vs 2 txs)

Harness: `crates/multivm/src/versions/vm_fast/mem_dos_stack_flood_vmfast.rs` (branch
`test/stack-flood-compound`, feature `mem-dos-flood-test`, `#[ignore]`d). Real `vm_fast::World` +
`MockStorage` + `VirtualMachine::run`, reusing the flood-only harness's machinery and its exact
rv32 accounting: `witness(mass) + program_cache(code_page + instrs 16->12 B) + heap-pin + stack`.
B_min(rv32) = 160 MiB. Dual cap: **768 MiB (verified current)** and **950 MiB (override)**.

Measured on the **#115 arm** (`vm2-115-116-125`, `zero()` = `slots[sc] = None`, i.e. **without**
#124) — deliberately, so the result cannot be attributed to #124.

## Why the stack pool is sticky (source-verified in vm2 `stack.rs`)

`StackPool::get()` calls `Stack::zero()`; `recycle()` does **not**. So a pooled stack keeps its
materialized sub-chunks until it is **re-popped**, and `zero()` is the only #115/#124 difference.
The flood far-calls victims *sequentially at depth ~1*, so it re-pops only the **top** pool slot and
never drains the deep pool phase 1 built — **on either arm**.

Measured arm delta (pure vm2, shallow/flood-shaped reuse wave, @20M): **#124 = 340 MiB vs
#115 = 336 MiB** — ~4 MiB (2 slots). Contrast a *deep* passive reuse wave, where #115 drains to
18 MiB and #124 keeps 351: that pattern is *not* the attacker's best move.
**=> The compounding term is a `StackPool` property, not a #124 regression.**

## Case A — 1 tx (phases share one gas budget N, so they compete)

`stack_nat` and `flood_nat` are native MiB; `tot_rv32` includes B_min = 160.

| N | g_rec | stack | flood | fp_rv32 | tot_rv32 | @768 | @950 |
|---|---|---|---|---|---|---|---|
| 15M | 0 (all-flood) | 0 | 461 | 517 | 677 | PASS +91 | PASS +273 |
| 15M | **2M** | 103 | 398 | 550 | **710** | PASS +58 | PASS +240 |
| 20M | 0 (all-flood) | 0 | 614 | 690 | 850 | FAIL −82 | PASS +100 |
| 20M | 1M | 60 | 582 | 713 | 873 | FAIL −105 | PASS +77 |
| 20M | **2M** | 103 | 552 | 722 | **882** | FAIL −114 | PASS +68 |
| 20M | 3M | 135 | 521 | 720 | 880 | FAIL −112 | PASS +70 |
| 20M | 5M | 182 | 461 | 699 | 859 | FAIL −91 | PASS +91 |
| 22M | 0 (all-flood) | 0 | 675 | 758 | 918 | FAIL −150 | PASS +32 |
| 22M | **2M** | 103 | 614 | 792 | **952** | FAIL −184 | **FAIL −2** |
| 25M | **2M** | 103 | 705 | 894 | **1054** | FAIL −286 | FAIL −104 |

The `g_rec = 0` rows reproduce the flood-only harness exactly (690 rv32 / +100 @950 at 20M) —
cross-validation that the two harnesses agree.

**The 1-tx optimum is NOT all-flood.** `pool(g)` is strongly sublinear (depth ~ g^0.56 under the
63/64 far-call decay: 64 MiB@1M, 107@2M, 139@3M, 259@10M, 340@20M => marginal 56/43/32/15/8 MiB per
1M gas) while the flood is linear at ~34.5 MiB/1M. Below ~2.5–3M gas the stack is the *steeper*
sink, so a small recursion prelude plus a shorter flood beats a pure flood by **+32 MiB** @20M
(882 vs 850). **This mildly violates the prior "all-flood is the max / sinks sub-additive" LP
conclusion.** It does not change the 20M verdict (@950 headroom 100 → 68), but it lowers the safe
1-tx ceiling.

## Case B — 2 txs (tx1 = full N on stack inflation, tx2 = full N on flood)

The pool lives on the `VirtualMachine`, so it survives tx boundaries; the two phases no longer
compete for gas.

| N per tx | stack | flood | fp_rv32 | tot_rv32 | @768 | @950 |
|---|---|---|---|---|---|---|
| 8M | 231 | 245 | 506 | 666 | PASS +102 | PASS +284 |
| 10M | 255 | 308 | 600 | 760 | PASS +8 | PASS +190 |
| 12M | 276 | 368 | 689 | 849 | FAIL −81 | PASS +101 |
| 15M | 302 | 461 | 819 | 979 | FAIL −211 | **FAIL −29** |
| 20M | 336 | 614 | 1026 | **1186** | FAIL −418 | FAIL −236 |
| 22M | 347 | 675 | 1105 | 1265 | FAIL −497 | FAIL −315 |
| 25M | 363 | 767 | 1225 | 1385 | FAIL −617 | FAIL −435 |

## Safe per-tx gas ceilings

| model | @768 MiB | @950 MiB |
|---|---|---|
| flood only (prior conclusion) | ~17.5M | ~22.9M |
| **1 tx, optimal split** (new) | ~16.5M | **~21.8M** |
| **2 txs** (new) | **~10M** | **~14M** |

## Reading

1. **Single-tx guarantee survives, slightly reduced.** At 20M the optimal split is 882 vs 850 for
   pure flood: still PASS @950 (+68), still FAIL @768. The @950 ceiling moves 22.9M → ~21.8M.
2. **Two txs break both caps at realistic gas.** 2×20M = 1186 MiB → FAIL @950 by 236. Even 2×15M
   fails @950 (−29). The "operator splits risky batches to 1 tx/batch" mitigation is therefore
   **load-bearing**, and it is load-bearing on **#115 too** — not because of #124.
3. **Scaling with more txs:** the pool term **saturates** after one materializing tx (pool = max
   concurrent depth ever reached × 2 MiB, and depth is per-tx gas-bounded, ~340 MiB @20M), while the
   flood term keeps growing **linearly with total batch gas** (unbounded `program_cache`, no #92).
   So k txs ≈ `340 MiB + 34.5 MiB/1M × (total flood gas)`: the flood dominates the tail, with the
   stack pool as a fixed additive floor on top.
4. **#124 is not the culprit and is not a regression vs production:** ordering is
   `master ≥ #124 ≥ #115` (918 ≥ 351 ≥ 18 MiB for a deep reuse wave @20M), and for the *worst-case*
   attack shape (shallow flood wave) all three arms are within ~4 MiB.

## Caveats

- Allocator counts **requested** bytes, not talc RSS/fragmentation (previously found negligible).
- Native is conservative for the stack (U256 slots are 32 B on both targets → 1:1) and the flood's
  instruction term is converted 16→12 B analytically, as in the flood-only harness.
- `B_min = 160 MiB` (rv32) is the only estimated term; use the `fp_rv32` column to re-score.
- Single-`VirtualMachine`, node-free (no bootloader): same fidelity trade-offs as the flood harness.

---

# Confirmation on the #124 arm + why the arm delta is only 4 MiB

## Case A / Case B, both arms (tot_rv32 MiB, B_min=160)

| N per tx | case | #115 (`vm2-115-116-125`) | #124 (`vm2-all-four`) | delta |
|---|---|---|---|---|
| 15M | A, optimal split (g_rec=2M) | 710 | 714 | 4 |
| 15M | **B (2 txs)** | 979 (FAIL @950 −29) | 983 (FAIL @950 −33) | **4** |
| 20M | A, all-flood | 850 | 850 | 0 |
| 20M | A, optimal split (g_rec=2M) | 882 | 886 | 4 |
| 20M | **B (2 txs)** | 1186 (FAIL @950 −236) | 1190 (FAIL @950 −240) | **4** |

Arm delta is a flat **4 MiB** everywhere. Every conclusion in this document holds on both arms.

## Why 4 MiB and not 333 MiB — proven, not argued

`#115` frees **lazily**: the free lives inside `Stack::zero()`, and `zero()` runs **only** on
`StackPool::get()` (a re-pop), **never** on `StackPool::recycle()` (the release). So:

* When phase 1's deep recursion unwinds, `recycle()` just pushes each materialized stack back.
  **Neither arm shrinks.** The pool is `D x 2 MiB` on `master`, `#124` and `#115` alike.
* The only memory `#115` can reclaim is `2 MiB x (number of DISTINCT pool slots later re-popped)`.
* `StackPool` is a `Vec` (`pop`/`push`), i.e. **LIFO**. A decommit flood runs at depth ~1, so it
  cycles only the **top** slot(s) — everything deeper is never touched, so `zero()` never runs on it
  and the `#115`/`#124` difference never gets a chance to apply.

Measured directly (`shallow_wave_pool_histogram`, per-slot chunk counts after a 200-far-call
flood-shaped phase 2 on a pool built by a 20M materializing recursion):

| arm | pooled stacks | materialized | **empty (freed)** | pool MiB | top-of-pool (last 8 slots) |
|---|---|---|---|---|---|
| #124 | 167 | 167 | **0** | 333.2 | `[4096 x 8]` |
| #115 | 167 | 165 | **exactly 2** | 329.2 | `[4096,4096,4096,4096,4096,4096,0,0]` |

The two freed slots are at the **top** of the pool — the ones the LIFO wave re-popped (the flood's
attacker frame + its victim frame). `333.2 - 329.2 = 4.0 MiB = 2 slots x 2 MiB`. That is the whole
arm delta.

**So the same two arms differ by 4 MiB or by 333 MiB purely as a function of the SHAPE of the later
workload:**

| phase-2 shape | distinct slots re-popped | #115 retained | #124 retained | delta |
|---|---|---|---|---|
| shallow / flood-shaped (depth ~1) | 2 | 329 (336 incl. live frames) | 333 (340) | **~4 MiB** |
| deep passive recursion (depth 457) | ~all 167 | **18** | **351** | **333 MiB** |

The attacker chooses the shape — and the shape that **maximizes total memory** is the flood
(shallow), because the flood is the far steeper sink (34.5 MiB per 1M gas, linear, vs the pool's
sublinear tail). The deep-recursion shape that would let `#115` reclaim is the attacker's *worse*
option. Hence the worst case is arm-independent, and `#124` is not what makes the compound attack
work.


---

# Revision 2 — self-review corrections

Two defects in the original measurement, both found by reviewing my own harness adversarially.

## Defect 1 — the phase-1 attack shape was suboptimal (understated the pool by 1.34–1.55x)

The materializer wrote its stack **before** recursing. That subtracts the per-frame write cost
*before* the 63/64 decay at every deeper level, so it compounds and caps depth (~166 frames @20M).
A frame that materializes **after its callee returns** gets the subtree's unspent gas back
(`naked_ret` credits `leftover_gas` to the caller), so the same budget materializes far more frames.

Measured, pure vm2 (`stack_sink_write_order`), pool MiB:

| gas | write before recursing (old) | write on unwind (attacker-optimal) | ratio |
|---|---|---|---|
| 2M | 106.7 | 165.5 | 1.55x |
| 15M | 305.7 | 425.5 | 1.39x |
| 20M | 339.9 | **462.7** | 1.36x |
| 25M | 366.9 | 491.6 | 1.34x |

This is near-optimal, not a stepping stone to something much larger: the analytic bound is
`writers in [d, D] <= g_d / cost_per_frame`, maximized by concentrating writers shallow, giving
~225 frames @20M; measured 216 (~4% off). The harness now defaults to this shape
(`MEM_DOS_WRITE_ORDER=in` selects the weaker one).

## Defect 2 — the driver under-funded the flood phase (1/64 far-call tax)

A far-call delivers only 63/64 of what the caller holds, so with a flat 300k of slack the flood
phase silently received less than its requested `g_flood` and decommitted **3 fewer victims** than
the flood-only run it was differenced against — biasing the isolated stack term. Now the driver
sizes its own gas (`driver_gas`), and **both cases assert `comb.lfd == f_only.lfd`**, so the
differencing can no longer be silently invalid. That assertion is what caught this.

## Corrected results (attacker-optimal shape, verified equal flood work)

### Case A — 1 tx, worst split over `g_rec` (tot_rv32 MiB, B_min=160)

| N | all-flood | worst split | worst `g_rec` | @768 | @950 |
|---|---|---|---|---|---|
| 12M | 573 | 686 | 4M | PASS +82 | PASS +264 |
| 15M | 677 | 790 | 4M | FAIL −22 | PASS +160 |
| 18M | 779 | 892 | 4M | FAIL −124 | PASS +58 |
| **20M** | 850 | **963** | **4M** | FAIL −195 | **FAIL −13** |

At 20M the split beats a pure flood by **+113 MiB** (963 vs 850), not the +32 measured before — so
the "all-flood is the max / sinks sub-additive" conclusion is violated by a wide margin, and the
optimum moved from `g_rec=2M` to `4M`. **A single 20M transaction no longer fits the 950 cap.**

### Case B — 2 txs (tot_rv32 MiB)

| N per tx | stack | flood | tot_rv32 | @768 | @950 |
|---|---|---|---|---|---|
| 12M | 393 | 368 | 966 | FAIL −198 | FAIL −16 |
| 15M | 422 | 461 | 1100 | FAIL −332 | FAIL −150 |
| 18M | 446 | 552 | 1225 | FAIL −457 | FAIL −275 |
| 20M | 459 | 614 | **1309** | FAIL −541 | FAIL −359 |

End-of-run residency for 2x20M is **1072 MiB** measured directly — over the cap on its own.

### Corrected safe per-tx ceilings

| model | @768 MiB | @950 MiB |
|---|---|---|
| flood only | ~17.5M | ~22.9M |
| 1 tx, optimal split — *was* | ~16.5M | ~21.8M |
| **1 tx, optimal split — corrected** | **~14.4M** | **~19.6M** |
| 2 txs — *was* | ~10M | ~14M |
| **2 txs — corrected** | **~7.5M** | **~11.8M** |

## Defect 3 (not a miscalculation — a missed reachability point)

`stack_pool` is a field of `VirtualMachine`; `VmSnapshot` carries only `state_snapshot`, and
`State::snapshot` does not include the pool. **So a transaction that reverts still leaves the pool
inflated.** The attacker never needs the inflating transaction to succeed, which widens the
reachable surface — the pool is not undone by rollback, only by the VM being dropped.

## What is still unverified

- k >= 3 txs still not measured directly (pool saturation + linear flood is an argument).
- The cycle cost of the phase-1 recursion is not measured; whether the 2^36 cycle limit binds before
  memory for the split attack is open.
- `B_min = 160 MiB` remains the only estimated term, and the corrected Case A margin (−13 @950) is
  now well inside its uncertainty — the verdict at 20M is "at the cap", not comfortably either side.
