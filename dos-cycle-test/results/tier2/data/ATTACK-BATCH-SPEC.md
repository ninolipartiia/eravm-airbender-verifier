# Worst-case 1-tx batch for the PR#92 re-decode cycle DoS

Goal: a single transaction that forces the guest past the ~2^36 (68,719,476,736)
prover-cycle budget purely through PR#92's unpriced program re-decode, making the
batch unprovable. This documents the batch shape and how each number is derived;
the measured per-decode cost that closes the loop is in `RESULTS.md`.

## Preconditions
- Build under test **contains #92** (bounded `ProgramCache`). Without it the same
  workload triggers the *memory* DoS instead. Since the drafts stack
  `main <- #92 <- #93 <- #94`, build on the **#94 tip (`6e95317`)** so #93/#94 are
  included; that is the "assume #92 lands" state.
- `#93` adds a per-miss `load_factory_dep` bytecode clone (measured add-on).
  `#94` touches only `decommit_code` (CodeOracle/EVM path), NOT the far-call
  `decommit`, so it does not affect this vector.

## Corpus (setup, amortised across prior batches — NOT the attack tx)
- **33 distinct contracts, each 2,097,120 B** (65,535 words; odd + ≤(1<<16)-1 →
  the on-chain max, `basic_types/src/bytecode.rs`). Generator: `gen_attack_contracts.py`.
- Max size is optimal: a far-call to an already-decommitted hash costs a flat
  183 ergs regardless of size, while `Program::new` cost grows with size (only the
  `.take(1<<16)` instruction decode saturates at 512 KiB; the `code_page` +
  `u64`-word passes scale to 2 MiB — **~1.83× a 512 KiB decode, measured**).
- Total 66.0 MiB > 64 MiB cache cap → a round-robin far-call cycle is 100% FIFO miss.
- Bytecode can be repetitive (compresses ~to nothing → cheap to publish as pubdata
  at deploy); instruction 0 must be a clean `ret.ok` so callees return without
  burning the passed 63/64 gas.

## The one transaction
- An L2 tx (or unskippable L1→L2 priority tx for the liveness framing) with
  `gas_limit = 80,000,000` (`TX_MAX_COMPUTE_GAS_LIMIT`).
- Calls a **driver contract** that loops: `for i in 0.. { far_call(contract[i % 33]) }`
  until out of gas. Each callee `ret`s immediately.

## Cycle arithmetic (all constants source-verified)
| step | value |
| --- | --- |
| far-call base price | 183 ergs (`2·4+1+150+20+2+1+1`) |
| decommit (first time only) | 4 ergs/word = 1 erg / 8 B; repeat = **0** |
| fill 64 MiB cache | 8,388,608 ergs |
| thrash budget | 80M − 8.39M ≈ 71.61M ergs |
| **miss-calls** | 71.61M / (183 + loop overhead) ≈ **286k–391k** |
| decode work / miss | full ~2 MiB `Program::new` (+ #93 clone) |
| prover ceiling | 2^36 ≈ 6.87×10^10 cycles |

**total_guest_cycles = miss_calls × cycles_per_2MiB_decode** → compare to 2^36.
Per-decode cost is measured on the real guest (`RESULTS.md`); miss-count band above.

## Node-route (definitive end-to-end, out of session scope)
This repo has no witness synthesizer, so a real `AirbenderVerifierInput` needs a
local Era node (`docs/generating-batches.md`): deploy the 33 contracts + driver
across contiguous batches (raise `max_pubdata_per_batch` or rely on compression),
submit the 80M-gas tx, `curl .../proof_inputs_no_lock/<N>` → `encode_batch` →
`attack.bin.gz`, then `cycle_bench --batch-files attack.bin.gz --app-bin-dir <#94⊕marker guest>`
and read `effective_cycles`.
