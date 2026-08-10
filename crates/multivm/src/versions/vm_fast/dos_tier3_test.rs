//! Tier 3-lite of the PR#92 re-decode cycle-DoS validation.
//!
//! Unlike Tier 1 (a standalone sim with a *verbatim copy* of the FIFO) this test drives the
//! **real** `ProgramCache` and the **real** `World::decommit` — the exact code vm2 calls on
//! every far call via `pay_for_decommit` (`world.decommit(code_key)`). It adds the one thing
//! Tiers 1–2 lack: a **bounded-vs-unbounded causation control** proving #92 is the cause.
//!
//! - Part A drives the real `World::decommit` (World + ProgramCache + bytecode_cache) over the
//!   worst-case attack pattern and times it — a cache-hit is ~µs, a re-decode is ~ms, so the
//!   per-call time shows the far-call path really re-decodes.
//! - Part B drives the real `ProgramCache` at cap = 64 MiB (#92) vs `usize::MAX` (main's
//!   unbounded cache). `World::decommit` runs `Program::new` iff `program_cache.get()` is
//!   `None`, so the ProgramCache **miss count == the re-decode count, exactly**. Bounded ⇒
//!   100% miss after fill; unbounded ⇒ 0 re-decodes. That contrast is the causation proof.
//!
//! Not covered here (node-gated, see dos-cycle-test/tiers/TIER3-LITE.md): full bootloader-
//! driven tx execution through `FastVmInstance`, which needs a synthesized valid tx + a
//! complete genesis/read-set (no in-repo witness producer). The far-call opcode's own erg
//! cost is accounted analytically in Tier 1.
//!
//! Slow (~a few seconds of real 2 MiB decodes); `#[ignore]`d. Run with:
//!   cargo test -p zksync_multivm --release dos_tier3 -- --ignored --nocapture

use std::collections::HashMap;
use std::time::Instant;

use zksync_types::{u256_to_h256, StorageKey, StorageValue, H256, U256};
use zksync_vm2::{testonly::TestWorld, Program, World as Vm2World};

use super::program_cache::ProgramCache;
use super::world::World;
use crate::interface::storage::ReadStorage;

const CAP_92: usize = 64 << 20; // #92's PROGRAM_CACHE_CAP_BYTES
const N: usize = 33; // 33 * 2 MiB = 66.0 MiB > 64 MiB cap
const WORDS: usize = 65_535; // max on-chain bytecode length (odd)
const BYTES: usize = WORDS * 32; // 2,097,120
const CYCLES: usize = 3; // cyclic passes (enough to show steady state)

/// Deterministic, content-independent max-size bytecode (decode is content-independent).
fn bytecode(seed: u8) -> Vec<u8> {
    let mut v = vec![0u8; BYTES];
    for (i, b) in v.iter_mut().enumerate() {
        *b = seed.wrapping_add((i % 251) as u8);
    }
    v
}

fn codes_and_hashes() -> (Vec<Vec<u8>>, Vec<U256>) {
    let codes = (0..N).map(|i| bytecode(i as u8 + 1)).collect();
    let hashes = (0..N).map(|i| U256::from(i as u64 + 1)).collect();
    (codes, hashes)
}

/// Part B: drive the REAL `ProgramCache` at `cap`, replaying `World::decommit`'s exact gate
/// (`get`; on `None` → `Program::new` → `insert`). Returns (fill_misses, thrash_misses,
/// thrash_calls). Miss == re-decode, by construction.
fn run_real_cache(cap: usize) -> (u64, u64, u64) {
    let (codes, hashes) = codes_and_hashes();
    let mut cache: ProgramCache<(), TestWorld<()>> = ProgramCache::new(HashMap::new(), cap);

    let mut fill_miss = 0u64;
    for i in 0..N {
        if cache.get(hashes[i]).is_none() {
            fill_miss += 1;
            let p: Program<(), TestWorld<()>> = Program::new(&codes[i], false);
            cache.insert(hashes[i], p);
        }
    }

    let (mut thrash_miss, mut calls) = (0u64, 0u64);
    for _ in 0..CYCLES {
        for i in 0..N {
            calls += 1;
            if cache.get(hashes[i]).is_none() {
                thrash_miss += 1;
                let p: Program<(), TestWorld<()>> = Program::new(&codes[i], false);
                cache.insert(hashes[i], p);
            }
        }
    }
    (fill_miss, thrash_miss, calls)
}

/// Minimal storage serving the 33 attack bytecodes via `load_factory_dep` (the only method
/// `World::decommit` needs). Non-panicking defaults for the rest.
#[derive(Debug)]
struct MockStorage {
    deps: HashMap<H256, Vec<u8>>,
}
impl ReadStorage for MockStorage {
    fn read_value(&mut self, _: &StorageKey) -> StorageValue {
        H256::zero()
    }
    fn is_write_initial(&mut self, _: &StorageKey) -> bool {
        true
    }
    fn load_factory_dep(&mut self, hash: H256) -> Option<Vec<u8>> {
        self.deps.get(&hash).cloned()
    }
    fn get_enumeration_index(&mut self, _: &StorageKey) -> Option<u64> {
        None
    }
}

/// Part A: drive the REAL `World::decommit` over the attack pattern; time the thrash phase.
fn run_real_world() -> (u64, f64) {
    let (codes, hashes) = codes_and_hashes();
    let deps: HashMap<H256, Vec<u8>> = (0..N)
        .map(|i| (u256_to_h256(hashes[i]), codes[i].clone()))
        .collect();
    let mut world: World<MockStorage, ()> = World::new(MockStorage { deps }, HashMap::new());

    for i in 0..N {
        let _ = Vm2World::decommit(&mut world, hashes[i]); // fill
    }
    let t = Instant::now();
    for _ in 0..CYCLES {
        for i in 0..N {
            let _ = Vm2World::decommit(&mut world, hashes[i]); // thrash
        }
    }
    let calls = (CYCLES * N) as u64;
    let ms_per_call = t.elapsed().as_secs_f64() * 1e3 / calls as f64;
    (calls, ms_per_call)
}

#[test]
#[ignore = "slow DoS demonstration (~seconds of real 2 MiB decodes); run with --ignored"]
fn dos_tier3_redecode_causation() {
    println!("\n=== Tier 3-lite — real ProgramCache / World::decommit (PR#92 cycle-DoS) ===");
    println!(
        "config: {N} contracts x {BYTES} B ({WORDS} words) = {:.1} MiB working set; #92 cap = {} MiB; {CYCLES} thrash passes",
        (N * BYTES) as f64 / (1 << 20) as f64,
        CAP_92 >> 20,
    );

    // ---- Part B: real ProgramCache, bounded (#92) vs unbounded (main) ----
    let (fb, tb, cb) = run_real_cache(CAP_92);
    let (fu, tu, cu) = run_real_cache(usize::MAX);
    println!("\n-- Part B: real ProgramCache (miss == re-decode, by construction) --");
    println!("  BOUNDED  (64 MiB, #92):   fill_misses={fb}/{N}  thrash_misses={tb}/{cb}  ({:.0}% thrash miss)", tb as f64 / cb as f64 * 100.0);
    println!("  UNBOUNDED(usize::MAX,main):fill_misses={fu}/{N}  thrash_misses={tu}/{cu}  ({:.0}% thrash miss)", tu as f64 / cu as f64 * 100.0);

    // ---- Part A: real World::decommit (full path incl. bytecode_cache) ----
    let (calls_a, ms_a) = run_real_world();
    println!("\n-- Part A: real World::decommit (full path) --");
    println!("  {calls_a} thrash far-calls, {ms_a:.2} ms/call (a cache HIT would be ~µs; ms-scale ⇒ real re-decode)");

    // ---- Causation assertions ----
    assert_eq!(tb, cb, "BOUNDED: every post-fill far-call must re-decode (100% miss)");
    assert_eq!(tu, 0, "UNBOUNDED: no re-decode after fill (this is main's behavior)");
    assert!(ms_a > 1.0, "real World::decommit re-decodes on the bounded cache (>1 ms/call)");

    println!("\nCAUSATION: #92's bounded cache turns {cb} far-calls into {tb} full re-decodes;");
    println!("the unbounded cache (main) does {tu}. #92 is the cause of the re-decode DoS.");
    println!("=== TIER 3-lite PASS ===\n");
}
