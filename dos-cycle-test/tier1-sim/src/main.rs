//! Tier 1 — native FIFO-economics simulation of the PR#92 re-decode cycle-DoS.
//!
//! Drives the *verbatim* #92 `ProgramCache` FIFO (copied from
//! `crates/multivm/src/versions/vm_fast/program_cache.rs` @ commit `1dc7ce8`) with the
//! real `zksync_vm2::Program::new` decode, over the worst-case attack access pattern
//! (33 distinct max-size contracts cycled far-call-by-far-call). It replays exactly what
//! `World::decommit` does per far call — `get`; on miss `Program::new` then `insert`
//! (which evicts) — and reports hit/miss/eviction/decode tallies, then derives the
//! budget-limited decode count and the 2^36 multiple (combined with Tier 2's measured
//! guest cyc/decode).
//!
//! It does NOT run the full ~390k real decodes (~1 h); it proves the miss RATE on a few
//! cycles and computes the COUNT analytically from the erg budget.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use primitive_types::U256;
use zksync_vm2::{testonly::TestWorld, Program};

/// Concrete program type for the sim (tracer `()`, world `TestWorld`).
type P = Program<(), TestWorld<()>>;

// ============================================================================
// VERBATIM #92 ProgramCache (program_cache.rs @ 1dc7ce8), generic collapsed to `P`.
// Logic — new/get/insert/evict_to_cap + program_bytes — is identical; only the type
// parameters are specialized. `evict_to_cap` additionally returns the eviction count.
// ============================================================================
const PROGRAM_CACHE_CAP_BYTES: usize = 64 << 20; // 64 MiB of bytecode

struct CacheEntry {
    program: P,
    evictable_bytes: Option<usize>,
}

struct ProgramCache {
    entries: HashMap<U256, CacheEntry>,
    fifo: VecDeque<U256>,
    evictable_bytes: usize,
    cap_bytes: usize,
}

impl ProgramCache {
    fn new(cap_bytes: usize) -> Self {
        // No pinned system contracts in the sim (they are irrelevant to the attack).
        Self {
            entries: HashMap::new(),
            fifo: VecDeque::new(),
            evictable_bytes: 0,
            cap_bytes,
        }
    }

    fn get(&self, hash: U256) -> Option<P> {
        self.entries.get(&hash).map(|entry| entry.program.clone())
    }

    fn insert(&mut self, hash: U256, program: P) -> usize {
        if self.entries.contains_key(&hash) {
            return 0;
        }
        let bytes = program_bytes(&program);
        self.entries.insert(
            hash,
            CacheEntry {
                program,
                evictable_bytes: Some(bytes),
            },
        );
        self.fifo.push_back(hash);
        self.evictable_bytes += bytes;
        self.evict_to_cap()
    }

    fn evict_to_cap(&mut self) -> usize {
        let mut evicted = 0;
        while self.evictable_bytes > self.cap_bytes {
            let Some(hash) = self.fifo.pop_front() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&hash) {
                self.evictable_bytes -= entry.evictable_bytes.unwrap_or(0);
                evicted += 1;
            }
        }
        evicted
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

fn program_bytes(program: &P) -> usize {
    program.code_page().len() * 32
}
// ============================================================================

#[derive(Default)]
struct Stats {
    hits: u64,
    misses: u64,
    decodes: u64,
    evictions: u64,
    decode_ns: u128,
}

/// Faithful replay of `World::decommit` (world.rs @ 1dc7ce8): cache hit → clone;
/// miss → `Program::new` (the forced re-decode) → insert (which may evict).
fn decommit(cache: &mut ProgramCache, hash: U256, code: &[u8], stats: &mut Stats) {
    if let Some(_p) = cache.get(hash) {
        stats.hits += 1;
        return;
    }
    stats.misses += 1;
    let t = Instant::now();
    let program: P = Program::new(code, false);
    stats.decode_ns += t.elapsed().as_nanos();
    stats.decodes += 1;
    stats.evictions += cache.insert(hash, program) as u64;
}

// ---- attack / cost constants (source-verified; see EXPECTATIONS.md) ----
const FAR_CALL_ERGS: u64 = 183;
const ERGS_PER_WORD_DECOMMIT: u64 = 4;
const TX_BUDGET_ERGS: u64 = 80_000_000;
const CYCLE_CEILING: f64 = 68_719_476_736.0; // 2^36
const TIER2_CYC_PER_DECODE: f64 = 47_434_425.0; // 2 MiB + #93 clone (measured)

const N_CONTRACTS: usize = 33; // 33 * 2 MiB = 69.15 MiB > 64 MiB cap
const MAX_WORDS: usize = 65_535; // max on-chain bytecode length in 32-byte words (odd)
const CONTRACT_BYTES: usize = MAX_WORDS * 32; // 2,097,120
const PROOF_CYCLES: usize = 5; // cyclic passes to prove steady-state miss rate

fn main() {
    println!("=== Tier 1 — native FIFO-economics simulation (PR#92 cycle-DoS) ===\n");
    println!(
        "config: {N_CONTRACTS} contracts x {CONTRACT_BYTES} B ({MAX_WORDS} words) = {:.2} MiB working set; cap = {} MiB",
        (N_CONTRACTS * CONTRACT_BYTES) as f64 / (1 << 20) as f64,
        PROGRAM_CACHE_CAP_BYTES >> 20,
    );

    // Deterministic, content-independent bytecode (decode work is content-independent).
    // 32-byte words; repetitive so it is cheap to "deploy" (compresses) yet fully decodes.
    let make_bytecode = |seed: u8| -> Vec<u8> {
        let mut v = vec![0u8; CONTRACT_BYTES];
        for (i, b) in v.iter_mut().enumerate() {
            *b = seed.wrapping_add((i % 251) as u8);
        }
        v
    };
    let codes: Vec<Vec<u8>> = (0..N_CONTRACTS).map(|i| make_bytecode(i as u8 + 1)).collect();
    let hashes: Vec<U256> = (0..N_CONTRACTS).map(|i| U256::from(i as u64 + 1)).collect();

    let mut cache = ProgramCache::new(PROGRAM_CACHE_CAP_BYTES);

    // ---- Phase 1: fill (each contract far-called once) ----
    let mut fill = Stats::default();
    for i in 0..N_CONTRACTS {
        decommit(&mut cache, hashes[i], &codes[i], &mut fill);
    }
    let steady = cache.len();
    println!("\n-- Phase 1 (fill): {N_CONTRACTS} first-time far-calls --");
    println!(
        "  misses={} hits={} decodes={} evictions={} | cache steady-state = {} entries ({} MiB)",
        fill.misses, fill.hits, fill.decodes, fill.evictions, steady,
        (steady * CONTRACT_BYTES) >> 20,
    );

    // ---- Phase 2: thrash (cyclic far-calls) ----
    let mut thrash = Stats::default();
    for _ in 0..PROOF_CYCLES {
        for i in 0..N_CONTRACTS {
            decommit(&mut cache, hashes[i], &codes[i], &mut thrash);
        }
    }
    let far_calls = (PROOF_CYCLES * N_CONTRACTS) as u64;
    let miss_rate = thrash.misses as f64 / far_calls as f64 * 100.0;
    let ns_per_decode = if thrash.decodes > 0 {
        thrash.decode_ns as f64 / thrash.decodes as f64
    } else {
        0.0
    };
    println!("\n-- Phase 2 (thrash): {PROOF_CYCLES} cyclic passes = {far_calls} far-calls --");
    println!(
        "  misses={} hits={} decodes={} evictions={} | MISS RATE = {:.1}%",
        thrash.misses, thrash.hits, thrash.decodes, thrash.evictions, miss_rate,
    );
    println!(
        "  native decode cost = {:.2} ms/decode (2 MiB, x86)",
        ns_per_decode / 1e6
    );

    // ---- Pass/fail on the miss-rate model ----
    let pass_missrate = thrash.hits == 0 && thrash.misses == far_calls;
    let pass_steady = steady == 32;
    let pass_evict = thrash.evictions == far_calls; // 1 eviction per post-fill access
    println!("\n-- Structural checks --");
    println!("  [{}] 100% post-fill miss rate (0 hits)", tick(pass_missrate));
    println!("  [{}] steady-state cache = 32 entries", tick(pass_steady));
    println!("  [{}] 1 eviction per thrash far-call", tick(pass_evict));

    // ---- Budget-limited decode count (analytic, from the erg model) ----
    let fill_ergs = N_CONTRACTS as u64 * (MAX_WORDS as u64 * ERGS_PER_WORD_DECOMMIT + FAR_CALL_ERGS);
    let thrash_budget = TX_BUDGET_ERGS - fill_ergs;
    println!("\n-- Budget model (one 80M-gas tx) --");
    println!(
        "  fill cost = {} ergs ({:.2}M); thrash budget = {} ergs ({:.2}M)",
        fill_ergs, fill_ergs as f64 / 1e6, thrash_budget, thrash_budget as f64 / 1e6
    );
    println!("  {:>14} {:>14} {:>16} {:>12}", "per-iter ergs", "misses", "total guest cyc", "x 2^36");
    for per_iter in [FAR_CALL_ERGS, 223, 250] {
        let misses = thrash_budget / per_iter;
        let total = misses as f64 * TIER2_CYC_PER_DECODE;
        println!(
            "  {:>14} {:>14} {:>16.3e} {:>11.1}x",
            per_iter, misses, total, total / CYCLE_CEILING
        );
    }
    let breakeven = (CYCLE_CEILING / TIER2_CYC_PER_DECODE).ceil() as u64;
    let breakeven_ergs = fill_ergs + breakeven * FAR_CALL_ERGS;
    println!(
        "\n  break-even: {} misses reach 2^36  ->  fill + {}*183 = {} ergs ({:.1}% of one 80M tx)",
        breakeven, breakeven, breakeven_ergs, breakeven_ergs as f64 / TX_BUDGET_ERGS as f64 * 100.0
    );

    // ---- Extrapolated native wall-time for the full attack (context only) ----
    let full_misses = thrash_budget / FAR_CALL_ERGS;
    println!(
        "\n  (native wall-time to actually run all {} decodes ~= {:.1} min at {:.1} ms each — why we extrapolate)",
        full_misses,
        full_misses as f64 * ns_per_decode / 1e9 / 60.0,
        ns_per_decode / 1e6
    );

    let overall = pass_missrate && pass_steady && pass_evict;
    println!("\n=== TIER 1 {} ===", if overall { "PASS" } else { "FAIL" });
}

fn tick(b: bool) -> &'static str {
    if b {
        "PASS"
    } else {
        "FAIL"
    }
}
