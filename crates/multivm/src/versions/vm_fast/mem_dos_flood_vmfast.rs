//! Node-free COMBINED memory-DoS flood measurement over the **real `vm_fast::World`**
//! (program_cache + bytecode_cache), not vm2's `TestWorld`.
//!
//! CONFIG measured: the verifier of THIS branch — #93 (storage-loaded bytecode is NOT re-cached in
//! `bytecode_cache`) with NO #92, so `program_cache` is an UNBOUNDED `HashMap` and its residency
//! grows linearly with the flood mass (no eviction cap). The vm2 version is orthogonal to this
//! sink: the heap-pin term is set by `materialize_decommit_page` (a code page, always dense), which
//! the sparse-stack/heap PRs don't touch — measured identical on stock v0.6.0 and the 4-PR bundle.
//! (Build against a #92-capped tree instead and the program_cache term saturates rather than
//! growing linearly — the harness reports whichever the compiled `World` does.)
//!
//! Why this exists: the `TestWorld` flood harness (`vm2/tests/mem_dos_flood_farcall.rs`) could
//! only measure the *heap-page* term of a far-call flood (k's `heap(1)`), because `TestWorld`
//! Arc-shares the decoded `Program` and has **no** program_cache — the vm_fast layer's
//! `code_page(1)+instructions(1.5 rv32)` terms live only on the real `World`. This harness drives
//! the real `World` (same `MockStorage` pattern as `dos_tier3_test.rs`) so all residency terms are
//! MEASURED together. Full single-tx footprint = witness(mass, held in the snapshot; #93 keeps only
//! this one copy) + program_cache(code_page 1x + instructions ~2x native / 1.5x rv32) + heap-pin
//! (materialized decommit pages, ~1x mass, globally pinned so they survive the callee panic and
//! persist to the root frame) ≈ 5.0x mass native / 4.5x rv32.
//!
//! Two rungs (both node-free; no witness producer, no bootloader):
//!  - Part A `run_decommit_flood`: drive `World::decommit` over N distinct S-byte cold bytecodes
//!    (exactly `dos_tier3`'s real-World path). Isolates the program_cache term (native) so Part B's
//!    peak can be split into program_cache vs heap-pin for the rv32 conversion.
//!  - Part B `run_vm_flood`: the full combined run — a real `vm2::VirtualMachine::run()` whose
//!    attacker far-calls the N victims once each (gas_to_pass=0; the callee OOG-panics, but the
//!    decommit+materialize already happened caller-side and the page is globally pinned). Captures
//!    program_cache + heap-pin together = the true single-run combined residency over the witness.
//!
//! Scope: measures ONE tx's peak (worst single-sink far-call decommit flood). Relies on the
//! operator splitting risky batches to 1 tx (decommit pages accumulate across *successful* txs).
//!
//! Gated behind the `mem-dos-flood-test` feature (off by default; it installs a tracking global
//! allocator) and `#[ignore]`d. Run (gas sweep defaults to 15/20/25M):
//!   ZKSYNC_USE_CUDA_STUBS=1 cargo test -p zksync_multivm --release \
//!     --features mem-dos-flood-test mem_dos_flood_vmfast -- --ignored --nocapture --test-threads=1
//! Knobs (env): MEM_DOS_GAS_SWEEP="15000000,20000000,25000000" (per-tx gas points),
//!   MEM_DOS_S=524288 (bytes/contract), MEM_DOS_BMIN=160 (estimated guest baseline, rv32 MiB).

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use zk_evm_1_5_2::zkevm_opcode_defs::system_params::DEPLOYER_SYSTEM_CONTRACT_ADDRESS_LOW;
use zksync_types::{
    u256_to_h256, AccountTreeId, Address, StorageKey, StorageValue, H160, H256, U256,
};
use zksync_vm2::{
    addressing_modes::{
        Arguments, CodePage, Immediate1, Register, Register1, Register2, RegisterAndImmediate,
    },
    interface::opcodes,
    Instruction, ModeRequirements, Predicate, Program, Settings, VirtualMachine, World as Vm2World,
};

use super::world::World;
use crate::interface::storage::ReadStorage;

// ------------------------------------------------------------------- tracking allocator
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Tracking;
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = System.alloc(l);
        if !p.is_null() {
            let now = LIVE.fetch_add(l.size(), Ordering::Relaxed) + l.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        System.dealloc(p, l);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = System.realloc(p, l, n);
        if !q.is_null() {
            LIVE.fetch_sub(l.size(), Ordering::Relaxed);
            let now = LIVE.fetch_add(n, Ordering::Relaxed) + n;
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        q
    }
}
#[global_allocator]
static A: Tracking = Tracking;


const ATTACKER: Address = H160([0x11; 20]);
const ATTACKER_HASH: u64 = 0xA77ac6e5;
const AA_HASH: u64 = 0xAA;
const FAR_CALL_COST: u32 = 183;
const RICH_COST: u32 = 6;

type W = World<MockStorage, ()>;
type Prog = Program<(), W>;

fn args(cost: u32) -> Arguments {
    Arguments::new(Predicate::Always, cost, ModeRequirements::none())
}

/// A distinct S-byte cold bytecode. Content-independent decode (per `dos_tier3`), first byte =
/// idx for distinctness so every far call is a program_cache MISS → `Program::new` → cap accounting.
fn blob(idx: u64, s: usize) -> Vec<u8> {
    let mut v = vec![0u8; s];
    let seed = (idx as u8).wrapping_add(1);
    for (i, b) in v.iter_mut().enumerate() {
        *b = seed.wrapping_add((i % 251) as u8);
    }
    v
}

fn deployer() -> Address {
    Address::from_low_u64_be(DEPLOYER_SYSTEM_CONTRACT_ADDRESS_LOW as u64)
}

/// The code-info word the far-call reads from `DEPLOYER` storage for a victim, matching
/// `TestWorld::new`'s encoding: byte[0]=1 (deployed marker), byte[2..=3]=code_len (words),
/// byte[24..]=distinct id. byte[1] is 0 (the far-call zeroes it to form the decommit hash), so
/// this value doubles as the `code_key` / factory-dep hash.
fn code_info(i: u64, words: usize) -> U256 {
    // code_len is a u16 (max on-chain bytecode = 65535 words = ~2 MiB); guard the silent wrap so a
    // large MEM_DOS_S can't produce a mis-sized code page.
    assert!(words <= u16::MAX as usize, "S too large: {words} words exceeds u16 code_len");
    let mut b = [0u8; 32];
    b[0] = 1;
    b[2..=3].copy_from_slice(&(words as u16).to_be_bytes());
    b[24..].copy_from_slice(&(i + 1).to_be_bytes());
    U256::from_big_endian(&b)
}

/// Minimal default-AA (`ret`): far-calls PAST the N victims land here; all dedup to one page.
fn aa_program() -> Prog {
    Program::from_raw(
        vec![Instruction::from_ret(Register1(Register::new(0)), None, args(RICH_COST))],
        vec![U256::zero()],
    )
}

/// Attacker: far-call victims base+1..=base+N once each, gas_to_pass=0; loop on panic. Counter in
/// caller heap (registers are cleared by far-call). Mirrors the TestWorld flood harness.
fn flood_loop(base: u64) -> Prog {
    let r0 = Register::new(0);
    let r_abi = Register::new(1);
    let r_dst = Register::new(3);
    let mut code = Vec::new();
    code.push(Instruction::from_add(
        CodePage(RegisterAndImmediate { immediate: 1, register: r0 }).into(),
        Register2(r0),
        Register1(r_dst).into(),
        args(RICH_COST),
        false,
        false,
    ));
    code.push(Instruction::from_heap_write(
        Immediate1(0).into(),
        Register2(r_dst),
        None,
        args(RICH_COST),
        false,
    ));
    code.push(Instruction::from_add(
        CodePage(RegisterAndImmediate { immediate: 0, register: r0 }).into(),
        Register2(r0),
        Register1(r_abi).into(),
        args(RICH_COST),
        false,
        false,
    ));
    code.push(Instruction::from_heap_read(
        Immediate1(0).into(),
        Register1(r_dst),
        None,
        args(RICH_COST),
    ));
    code.push(Instruction::from_add(
        Immediate1(1).into(),
        Register2(r_dst),
        Register1(r_dst).into(),
        args(RICH_COST),
        false,
        false,
    ));
    code.push(Instruction::from_heap_write(
        Immediate1(0).into(),
        Register2(r_dst),
        None,
        args(RICH_COST),
        false,
    ));
    code.push(Instruction::from_far_call::<opcodes::Normal>(
        Register1(r_abi),
        Register2(r_dst),
        Immediate1(8),
        false,
        false,
        args(FAR_CALL_COST),
    ));
    code.push(Instruction::from_jump(Immediate1(2).into(), Register1(Register::new(4)), args(RICH_COST)));
    code.push(Instruction::from_jump(Immediate1(2).into(), Register1(Register::new(4)), args(RICH_COST)));
    Program::from_raw(code, vec![U256::zero(), U256::from(base)])
}

/// Storage serving the flood: code-key(addr) → victim hash (so far-call resolves it) and
/// load_factory_dep(hash) → the S-byte bytecode. Non-panicking zero defaults for everything else.
#[derive(Debug)]
struct MockStorage {
    code_keys: HashMap<StorageKey, StorageValue>,
    deps: HashMap<H256, Vec<u8>>,
    lfd_calls: usize,
}
impl ReadStorage for MockStorage {
    fn read_value(&mut self, key: &StorageKey) -> StorageValue {
        self.code_keys.get(key).copied().unwrap_or_else(H256::zero)
    }
    fn is_write_initial(&mut self, _: &StorageKey) -> bool {
        true
    }
    fn load_factory_dep(&mut self, hash: H256) -> Option<Vec<u8>> {
        self.lfd_calls += 1;
        self.deps.get(&hash).cloned()
    }
    fn get_enumeration_index(&mut self, _: &StorageKey) -> Option<u64> {
        None
    }
}

/// Storage key the far-call reads for `address`'s code info: `(DEPLOYER, address_as_u256)`.
fn code_info_key(addr_u256: U256) -> StorageKey {
    StorageKey::new(AccountTreeId::new(deployer()), u256_to_h256(addr_u256))
}

fn build_storage(n: u64, s: usize, base: u64) -> MockStorage {
    let words = s / 32;
    let mut code_keys = HashMap::new();
    let mut deps = HashMap::new();
    for i in 0..n {
        // far-call target address == r_dst == base+1+i (only low bytes set), so the DEPLOYER
        // storage key uses that value directly.
        let ci = code_info(i, words);
        code_keys.insert(code_info_key(U256::from(base + 1 + i)), u256_to_h256(ci));
        deps.insert(u256_to_h256(ci), blob(i, s)); // decommit hash == code_info (byte[1]=0)
    }
    MockStorage { code_keys, deps, lfd_calls: 0 }
}

struct Out {
    peak: usize,
    baseline: usize,
    lfd: usize,
    extra: String,
}

/// Part A — decommit-only flood over the REAL World. Isolates the program_cache term + cap.
fn run_decommit_flood(n: u64, s: usize, base: u64) -> Out {
    let storage = build_storage(n, s, base);
    let mut world: W = World::new(storage, HashMap::new());

    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);

    let words = s / 32;
    for i in 0..n {
        let _ = Vm2World::decommit(&mut world, code_info(i, words));
    }
    let peak = PEAK.load(Ordering::Relaxed);
    // bytecode_cache stays empty (#93: storage-served bytecodes are deliberately NOT cached); the
    // program_cache (UNBOUNDED HashMap here — no #92) holds the decoded programs, growing with N.
    let bc = world.bytecode_cache.len();
    Out {
        peak: peak.saturating_sub(baseline),
        baseline,
        lfd: 0,
        extra: format!("bytecode_cache={bc} entries (expected 0 — storage-served not cached)"),
    }
}

/// Part B — full combined run: real VirtualMachine over the real World. Captures heap-pin +
/// capped program_cache together.
fn run_vm_flood(n: u64, s: usize, gas: u32, base: u64) -> Out {
    let storage = build_storage(n, s, base);
    let mut pinned = HashMap::new();
    pinned.insert(U256::from(ATTACKER_HASH), flood_loop(base));
    pinned.insert(U256::from(AA_HASH), aa_program());
    let mut world: W = World::new(storage, pinned);

    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);

    let aa_bytes: [u8; 32] = u256_to_h256(U256::from(AA_HASH)).to_fixed_bytes();
    let mut vm = VirtualMachine::new(
        ATTACKER,
        flood_loop(base),
        Address::zero(),
        &[],
        gas,
        Settings { default_aa_code_hash: aa_bytes, evm_interpreter_code_hash: [0; 32], hook_address: 0 },
    );
    let end = vm.run(&mut world, &mut ());
    let peak = PEAK.load(Ordering::Relaxed);
    Out {
        peak: peak.saturating_sub(baseline),
        baseline,
        lfd: world.storage.lfd_calls,
        extra: format!("end={end:?}"),
    }
}

#[test]
#[ignore = "slow node-free combined memory-DoS flood over the real vm_fast World; run with --ignored"]
fn mem_dos_flood_vmfast() {
    let s: usize = std::env::var("MEM_DOS_S").ok().and_then(|v| v.parse().ok()).unwrap_or(524288);
    let base: u64 = 0x10000;
    // GAS sweep (default 15M/20M/25M), env-overridable: MEM_DOS_GAS_SWEEP="15000000,20000000,25000000".
    let gas_targets: Vec<u64> = std::env::var("MEM_DOS_GAS_SWEEP")
        .ok()
        .map(|v| v.split(',').filter_map(|x| x.trim().parse::<u64>().ok()).collect::<Vec<_>>())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vec![15_000_000, 20_000_000, 25_000_000]);
    // The ONLY estimated term: the adversarial-minimal guest baseline (bootloader + system contracts
    // + tree + non-flood input), rv32 MiB. Everything else below is MEASURED. Override: MEM_DOS_BMIN.
    let b_min_rv32: f64 = std::env::var("MEM_DOS_BMIN").ok().and_then(|v| v.parse().ok()).unwrap_or(160.0);
    let per_gas = (s as u64 / 8) + FAR_CALL_COST as u64 + 60; // decommit(S/8) + far-call + loop
    let mb = |bytes: f64| bytes / (1u64 << 20) as f64;
    let cap950 = 950.0_f64;

    println!("\n=== worst-case far-call flood @ {{15,20,25}}M gas — verifier #93 (no #92, UNBOUNDED program_cache); vm2-version-independent ===");
    println!("S={} KiB/contract. Footprint (MEASURED) = witness(mass, held in snapshot; #93=only copy) + program_cache(decoded, unbounded) + heap-pin(materialized pages).", s / 1024);
    println!("rv32: instructions 16->12 B/instr; code_page & heap raw-bytes unchanged. B_min(guest baseline, rv32) = {:.0} MiB [ONLY estimated term]. CAP = 950 MiB.", b_min_rv32);
    println!("{:>6} {:>5} {:>7} | {:>8} {:>8} | {:>9} | {:>9} | {:>10}",
        "gasM", "N", "mass", "fp_nat", "fp_rv32", "hdrm950", "tot_rv32", "verdict950");

    for &g in &gas_targets {
        let n = (g / per_gas).max(1);
        let mass = (n as usize * s) as f64;
        let gas = (n * per_gas + 300_000).min(u32::MAX as u64) as u32;
        let a = run_decommit_flood(n, s, base); // program_cache native (isolated)
        let b = run_vm_flood(n, s, gas, base); // program_cache + heap-pin native (combined run)
        let pcache_nat = a.peak as f64;
        let heap_nat = (b.peak as f64 - a.peak as f64).max(0.0);
        let instr_nat = (pcache_nat - mass).max(0.0); // code_page == mass; rest is decoded instructions
        let pcache_rv32 = mass + instr_nat * (12.0 / 16.0);
        let fp_nat = b.baseline as f64 + b.peak as f64; // witness + pcache + heap
        let fp_rv32 = mass + pcache_rv32 + heap_nat; // witness & heap raw-bytes unchanged
        let fp_rv32_mib = mb(fp_rv32);
        let headroom = cap950 - fp_rv32_mib; // budget left for the guest baseline
        let tot = fp_rv32_mib + b_min_rv32;
        let verdict = if tot < cap950 {
            format!("PASS +{:.0}", cap950 - tot)
        } else {
            format!("FAIL -{:.0}", tot - cap950)
        };
        println!("{:>6.0} {:>5} {:>7.0} | {:>8.0} {:>8.0} | {:>9.0} | {:>9.0} | {:>10}",
            g as f64 / 1e6, n, mb(mass), mb(fp_nat), fp_rv32_mib, headroom, tot, verdict);
        println!("   decomp native MiB: witness {:.0} + program_cache {:.0} + heap-pin {:.0}  (lfd={}, {})",
            mb(mass), mb(pcache_nat), mb(heap_nat), b.lfd, b.extra);
    }
    println!("\nhdrm950 = 950 - fp_rv32 = MiB left for the guest baseline; PASS iff that exceeds real B_min. tot_rv32 uses B_min={:.0}.", b_min_rv32);
    println!("Unbounded program_cache ⇒ footprint grows LINEARLY with gas; #92 (excluded here) would cap it.");
}
