# Tier 1 results — native FIFO-economics simulation

Run: `dos-cycle-test/tier1-sim`, release, vm2 v0.6.0. Raw output: `run.log`.

## Outcome: **PASS** — all structural checks + budget model matched expectations.

### Structural (the miss-rate model)
| check | expected | measured | verdict |
| --- | --- | --- | --- |
| working set vs cap | 66.0 MiB > 64 MiB | 66.0 MiB (33 × 2,097,120 B) | ✓ |
| fill: cache steady state | 32 entries (33rd evicts 1st) | 32 entries, 1 eviction | ✓ |
| thrash miss rate | 100% (0 hits) | 165/165 misses, 0 hits | ✓ **PASS** |
| evictions per thrash call | 1 | 1 (165/165) | ✓ |

The 100% miss rate is the load-bearing claim and it holds structurally: after the fill
evicts contract #1, every cyclic access lands on a just-evicted slot. FIFO = LRU here.

### Budget model (one 80M-gas tx), combined with Tier 2's 47.43M cyc/decode
| per-iter ergs | misses | total guest cycles | × 2³⁶ |
| --- | --- | --- | --- |
| 183 (bare far-call) | 389,854 | 1.849×10¹³ | **269×** |
| 223 (+loop) | 319,925 | 1.518×10¹³ | **221×** |
| 250 (+loop) | 285,373 | 1.354×10¹³ | **197×** |

- Fill cost = **8.66M ergs** (matches the ~8.66M prediction), leaving 71.34M to thrash.
- Predicted miss count ~285k–390k → **measured band 285,373–389,854.** ✓
- **Break-even: 1,449 misses → 8.92M ergs = 11.2% of one 80M tx.** (matches the ~1,449 /
  ~11% prediction.)

### Expectation vs result
Every Tier 1 pre-registration held. Crucially, Tier 1 (the **count/economics**, derived
independently from the erg model + real FIFO) and Tier 2 (the **per-decode cost**, measured
in real guest cycles) were produced by entirely separate methods, yet their product lands
on the **same 197–269× over 2³⁶** and the **same ~1,449-miss / ~11%-of-a-tx break-even**.
Two independent halves agreeing is strong corroboration.

### Notes / limitations
- Native decode timing here = **7.47 ms/decode** (2 MiB, x86); the scratchpad probe earlier
  showed ~11 ms. This is native-host jitter/opt variance and is **not** the metric — the
  headline uses Tier 2's rv32 guest cycles. Native time only contextualizes why we
  extrapolate the ~390k-decode count (~48 min of real decodes) rather than run it.
- The FIFO is a verbatim copy of `program_cache.rs` @ `1dc7ce8`; the decode + eviction
  accounting (`code_page().len()*32`) use the real vm2 library.
- Omits far-call/`pay_for_decommit`/callee overhead → the "misses" column is an upper
  bound on count at a given per-iter erg cost; Tier 3-lite will pin the real per-iter cost.

### Adjustment to plan
None needed — Tier 1 confirms the count half. Proceed to consolidate Tier 2 (done) and
then attempt Tier 3-lite (native whole-VM, to fold in far-call overhead and exercise the
real `ProgramCache` inside a bootloader-driven run).
