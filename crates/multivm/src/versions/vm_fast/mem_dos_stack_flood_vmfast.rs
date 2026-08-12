//! Whole-VM memory-DoS harness — **stack pool + decommit flood, CO-RESIDENT** over the real
//! `vm_fast::World`. Sibling of `mem_dos_flood_vmfast.rs` (the flood-only harness); reuses its
//! `MockStorage`/`code_info`/`blob`/`flood_loop` machinery via `super::mem_dos_flood_vmfast`.
//!
//! # What this measures and why the flood-only harness does not cover it
//!
//! The flood-only harness dismisses the stack in one clause ("stacks gas-bounded and neutralized
//! by #115") because its victims are called with `gas_to_pass = 0` and OOG-panic before writing
//! any stack. This harness adds a **phase 1** that deliberately inflates the VM's `StackPool`, and
//! then runs the flood **on top of it**, so the two sinks are resident at the same instant.
//!
//! # The mechanism that makes the pool sticky (verified at source in vm2 `stack.rs`)
//!
//! `StackPool::get()` calls `Stack::zero()`; `StackPool::recycle()` does NOT. So:
//!   * a pooled stack keeps its materialized sub-chunks until it is **re-popped**;
//!   * `zero()` is the ONLY difference between #115 (`slots[sc] = None`, frees) and #124
//!     (`chunk.fill(0)`, clears in place and keeps).
//!
//! The flood far-calls victims **sequentially at depth ~1**, so it re-pops only the *top* pool
//! slot. It therefore never drains the deep pool phase 1 built — **on either arm**. That makes the
//! combined footprint essentially vm2-arm-independent (measured: #115 vs #124 differ by ~2 slots
//! = ~4 MiB), i.e. this is a property of `StackPool` never shrinking, NOT a #124 regression.
//!
//! # Transaction model (the axis this harness sweeps)
//!
//! The pool lives on the `VirtualMachine`, so it persists across transactions within a batch. The
//! driver's two sequential far-calls model two txs from the bootloader frame:
//!   * **1 tx** (`Case A`): both phases share ONE gas budget N — they COMPETE for gas.
//!   * **2 tx** (`Case B`): each phase gets its own full N — no competition. This is the case the
//!     "operator splits risky batches to 1 tx/batch" mitigation is load-bearing for.
//!
//! Because `pool(g)` is strongly **sublinear** (depth ~ g^0.56 under the 63/64 far-call gas decay)
//! while the flood is **linear** (~34.5 MiB per 1M gas, rv32), the stack's *marginal* return beats
//! the flood's below ~2.5-3M gas. So the 1-tx optimum is NOT all-flood, and Case A sweeps the
//! split to find it.
//!
//! # Accounting
//!
//! Mirrors the flood harness: `fp = witness(mass, the baseline) + peak-above-baseline`, where the
//! peak now also contains the stack pool. The stack term is isolated by differencing against a
//! flood-only run at the same victim count, and converts **1:1 native -> rv32** (U256 slots are
//! 32 B on both targets); only the flood's decoded-instruction term shrinks 16->12 B.
//!
//! Run (feature-gated, `#[ignore]`d; one `#[global_allocator]` per binary lives in the flood file):
//! ```text
//! ZKSYNC_USE_CUDA_STUBS=1 cargo test -p zksync_multivm --release \
//!   --features mem-dos-flood-test mem_dos_stack_flood -- --ignored --nocapture --test-threads=1
//! ```
//! Env: `MEM_DOS_S` (bytes/contract), `MEM_DOS_BMIN` (rv32 guest baseline MiB),
//! `MEM_DOS_N_SWEEP` (per-tx gas budgets), `MEM_DOS_REC_SWEEP` (phase-1 gas points for Case A).

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use zksync_types::{u256_to_h256, Address, H160, U256};
use zksync_vm2::{
    addressing_modes::{
        AbsoluteStack, CodePage, Immediate1, Register, Register1, Register2, RegisterAndImmediate,
    },
    interface::opcodes,
    Instruction, Program, Settings, VirtualMachine,
};

use super::mem_dos_flood_vmfast::{
    aa_program, args, build_storage, code_info, code_info_key, flood_loop, per_gas_for,
    run_decommit_flood, run_vm_flood_n, Out, AA_HASH, ATTACKER, ATTACKER_HASH, FAR_CALL_COST, LIVE,
    PEAK, RICH_COST,
};
use super::world::World;

type W = World<super::mem_dos_flood_vmfast::MockStorage, ()>;
type Prog = Program<(), W>;

/// Entry frame: sequences phase 1 (stack inflation) then phase 2 (flood).
const DRIVER: Address = H160([0x21; 20]);
/// Phase 1 target: self-recursive stack materializer.
const MAT: Address = H160([0x31; 20]);
/// Distinct code-info ids for the two pinned attacker programs (far above any victim id).
const MAT_ID: u64 = 0x7000_0001;
const ATT_ID: u64 = 0x7000_0002;
/// Sub-chunks in a `Stack` (2^16 slots / 16 slots per sub-chunk) — one write per sub-chunk
/// materializes the whole 2 MiB stack.
const NUM_SUBCHUNKS: u16 = 4096;

fn abi_gas(gas_to_pass: u32) -> U256 {
    let mut abi = U256::zero();
    abi.0[3] = gas_to_pass as u64;
    abi
}

fn load_codepage(imm: u16, dst: Register) -> Instruction<(), W> {
    let r0 = Register::new(0);
    Instruction::from_add(
        CodePage(RegisterAndImmediate {
            immediate: imm,
            register: r0,
        })
        .into(),
        Register2(r0),
        Register1(dst).into(),
        args(RICH_COST),
        false,
        false,
    )
}

/// Phase 1: self-recursive far-call to `MAT`, materializing every sub-chunk of each frame's stack
/// (one write per 16-slot sub-chunk = 2 MiB/frame). Passes `u32::MAX`, so each level takes 63/64 of
/// what remains.
///
/// `write_on_unwind` picks WHEN a frame materializes, and it matters a lot:
///   * `false` — before recursing. The write cost is then subtracted *before* the 63/64 decay at
///     every deeper level, so it compounds and caps depth (~166 frames @20M).
///   * `true`  — after its callee returns. `naked_ret` credits the callee's `leftover_gas` back to
///     the caller, so the subtree hands its unspent gas back and the same budget materializes far
///     more frames (~216 @20M, i.e. **1.36x more pool**). Registers are clobbered by the return,
///     but the writes only use r0 (hardwired zero), so they are still valid there.
/// Default is `true`: it is the attacker-optimal shape, and measured within ~4% of the analytic
/// bound (writers in [d, D] <= g_d / cost_per_frame, maximized shallow => ~225 frames @20M).
fn materializer(write_on_unwind: bool) -> Prog {
    let r0 = Register::new(0);
    let r_abi = Register::new(1);
    let r_dst = Register::new(2);
    let writes = |code: &mut Vec<Instruction<(), W>>| {
        for j in 0..NUM_SUBCHUNKS {
            code.push(Instruction::from_add(
                Register1(r0).into(),
                Register2(r0),
                AbsoluteStack(RegisterAndImmediate {
                    immediate: j.saturating_mul(16),
                    register: r0,
                })
                .into(),
                args(RICH_COST),
                false,
                false,
            ));
        }
    };
    let mut code = vec![load_codepage(0, r_abi), load_codepage(1, r_dst)];
    if write_on_unwind {
        // 2: recurse first; both success and OOG land on the writes at index 3.
        code.push(Instruction::from_far_call::<opcodes::Normal>(
            Register1(r_abi),
            Register2(r_dst),
            Immediate1(3),
            false,
            false,
            args(FAR_CALL_COST),
        ));
        writes(&mut code);
        code.push(Instruction::from_ret(Register1(r0), None, args(RICH_COST)));
    } else {
        writes(&mut code);
        let handler_idx = 2 + NUM_SUBCHUNKS + 2; // far, ret, [handler]
        code.push(Instruction::from_far_call::<opcodes::Normal>(
            Register1(r_abi),
            Register2(r_dst),
            Immediate1(handler_idx),
            false,
            false,
            args(FAR_CALL_COST),
        ));
        code.push(Instruction::from_ret(Register1(r0), None, args(RICH_COST)));
        code.push(Instruction::from_ret(Register1(r0), None, args(RICH_COST)));
    }
    let dst = U256::from_big_endian(MAT.as_bytes());
    Program::from_raw(code, vec![abi_gas(u32::MAX), dst])
}

/// Driver: far-call `MAT` with `g_rec`, then far-call the flood attacker with `g_flood`.
/// The reload between phases is required: a far-call clobbers the caller's registers on return.
fn driver(g_rec: u32, g_flood: u32) -> Prog {
    let r0 = Register::new(0);
    let (r_abi, r_dst) = (Register::new(1), Register::new(2));
    let mut code = vec![load_codepage(0, r_abi), load_codepage(1, r_dst)];
    // 2: phase 1; on OOG fall through to phase 2 (index 3).
    code.push(Instruction::from_far_call::<opcodes::Normal>(
        Register1(r_abi),
        Register2(r_dst),
        Immediate1(3),
        false,
        false,
        args(FAR_CALL_COST),
    ));
    // 3,4: reload for phase 2.
    code.push(load_codepage(2, r_abi));
    code.push(load_codepage(3, r_dst));
    // 5: phase 2 (flood); on OOG fall through to the final ret (index 6).
    code.push(Instruction::from_far_call::<opcodes::Normal>(
        Register1(r_abi),
        Register2(r_dst),
        Immediate1(6),
        false,
        false,
        args(FAR_CALL_COST),
    ));
    code.push(Instruction::from_ret(Register1(r0), None, args(RICH_COST)));
    Program::from_raw(
        code,
        vec![
            abi_gas(g_rec),
            U256::from_big_endian(MAT.as_bytes()),
            abi_gas(g_flood),
            U256::from_big_endian(ATTACKER.as_bytes()),
        ],
    )
}

/// Combined run: phase 1 inflates the pool, phase 2 floods `n` distinct bytecodes on top of it.
/// `vm_gas` is the whole driver frame's budget — set it to N for the 1-tx case and to ~2N for the
/// 2-tx case.
fn run_stack_then_flood(
    n: u64,
    s: usize,
    base: u64,
    g_rec: u32,
    g_flood: u32,
    vm_gas: u32,
    write_on_unwind: bool,
) -> Out {
    let words = s / 32;
    let mut storage = build_storage(n, s, base);
    // Wire the two pinned attacker programs so far-calls to them resolve (an unwired address would
    // silently fall through to the default AA and the phase would be a no-op).
    let mat_info = code_info(MAT_ID, words);
    let att_info = code_info(ATT_ID, words);
    storage.code_keys.insert(
        code_info_key(U256::from_big_endian(MAT.as_bytes())),
        u256_to_h256(mat_info),
    );
    storage.code_keys.insert(
        code_info_key(U256::from_big_endian(ATTACKER.as_bytes())),
        u256_to_h256(att_info),
    );
    // Pinned => program_cache hit, so neither attacker program adds a decode term to the flood's.
    let mut pinned = HashMap::new();
    pinned.insert(mat_info, materializer(write_on_unwind));
    pinned.insert(att_info, flood_loop(base));
    pinned.insert(U256::from(ATTACKER_HASH), flood_loop(base));
    pinned.insert(U256::from(AA_HASH), aa_program());
    let mut world: W = World::new(storage, pinned);

    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);

    let mut vm = VirtualMachine::new(
        DRIVER,
        driver(g_rec, g_flood),
        Address::zero(),
        &[],
        vm_gas,
        Settings {
            default_aa_code_hash: u256_to_h256(U256::from(AA_HASH)).to_fixed_bytes(),
            evm_interpreter_code_hash: [0; 32],
            hook_address: 0,
        },
    );
    let end = vm.run(&mut world, &mut ());
    let peak = PEAK.load(Ordering::Relaxed);
    let lfd = world.storage.lfd_calls;
    // Read residency while the VM (hence the pool) is still alive.
    let retained = LIVE.load(Ordering::Relaxed).saturating_sub(baseline);
    drop(vm);
    Out {
        peak: peak.saturating_sub(baseline),
        baseline,
        lfd,
        extra: format!(
            "end={end:?} retained_MiB={:.1}",
            retained as f64 / (1u64 << 20) as f64
        ),
    }
}

/// rv32 footprint, same decomposition as the flood-only harness:
///   witness(mass) + program_cache(code_page == mass, decoded instructions 16->12 B) + heap-pin
///   + stack pool (raw U256 slot bytes: 1:1 native->rv32).
/// Returns `(fp_rv32_bytes, heap_nat_bytes)`.
fn fp_rv32_of(mass: f64, pcache_nat: f64, flood_nat: f64, stack_nat: f64) -> (f64, f64) {
    let heap_nat = (flood_nat - pcache_nat).max(0.0);
    let instr_nat = (pcache_nat - mass).max(0.0);
    let pcache_rv32 = mass + instr_nat * (12.0 / 16.0);
    (mass + pcache_rv32 + heap_nat + stack_nat, heap_nat)
}

/// Gas for the driver frame delivering `total` to its phases. A far-call passes only 63/64 of what
/// the caller holds, so without >= g_flood/63 of headroom the flood phase runs short of its
/// requested budget and decommits fewer victims than the reference it is differenced against — the
/// `lfd` assertions below exist to catch exactly that. The headroom is harness scaffolding: a real
/// attacker's entry frame would run a phase itself rather than delegate, paying no such tax.
fn driver_gas(total: u64) -> u32 {
    (total + total / 32 + 300_000).min(u32::MAX as u64) as u32
}

const MIB: f64 = (1u64 << 20) as f64;
fn mb(bytes: f64) -> f64 {
    bytes / MIB
}

#[test]
#[ignore = "slow node-free combined stack-pool + decommit-flood memory-DoS; run with --ignored"]
fn mem_dos_stack_flood_vmfast() {
    let s: usize = std::env::var("MEM_DOS_S")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(524288);
    let b_min_rv32: f64 = std::env::var("MEM_DOS_BMIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(160.0);
    let parse_sweep = |var: &str, dflt: Vec<u64>| -> Vec<u64> {
        std::env::var(var)
            .ok()
            .map(|v| {
                v.split(',')
                    .filter_map(|x| x.trim().parse::<u64>().ok())
                    .collect::<Vec<_>>()
            })
            .filter(|v: &Vec<u64>| !v.is_empty())
            .unwrap_or(dflt)
    };
    let n_sweep = parse_sweep("MEM_DOS_N_SWEEP", vec![20_000_000]);
    let rec_sweep = parse_sweep(
        "MEM_DOS_REC_SWEEP",
        vec![0, 1_000_000, 2_000_000, 3_000_000, 5_000_000],
    );
    let base: u64 = 0x10000;
    let per_gas = per_gas_for(s);
    // Attacker-optimal phase-1 shape by default; set MEM_DOS_WRITE_ORDER=in for the weaker one.
    let write_on_unwind = std::env::var("MEM_DOS_WRITE_ORDER").map_or(true, |v| v != "in");

    println!("\n=== stack pool + decommit flood, CO-RESIDENT (real vm_fast::World) ===");
    println!(
        "S={} KiB/contract, per_victim_gas={per_gas}. B_min(rv32)={b_min_rv32:.0} MiB.",
        s / 1024
    );
    println!("DUAL CAP: 768 MiB (verified current) and 950 MiB (override).");
    println!("stack term converts 1:1 native->rv32; flood instructions 16->12 B.");
    println!(
        "phase-1 shape: materialize {} (write_on_unwind={write_on_unwind})",
        if write_on_unwind {
            "ON UNWIND (attacker-optimal)"
        } else {
            "before recursing (weaker)"
        }
    );

    // Reference: flood-ONLY at each N (g_rec = 0), the prior 'all-flood is the max' baseline.
    for &big_n in &n_sweep {
        println!(
            "\n--------- per-tx gas budget N = {:.0}M ---------",
            big_n as f64 / 1e6
        );

        // ---- Case A: ONE tx. Phases share N, so they compete for gas. ----
        println!("\n[Case A] 1 tx: phase1 + phase2 share N (they COMPETE for gas)");
        println!(
            "{:>8} {:>8} {:>5} | {:>9} {:>9} {:>9} | {:>9} | {:>13} {:>13}",
            "g_recM",
            "g_fldM",
            "N_vic",
            "stack_nat",
            "flood_nat",
            "fp_rv32",
            "tot_rv32",
            "verdict768",
            "verdict950"
        );
        let mut best: Option<(f64, u64)> = None;
        for &g_rec in &rec_sweep {
            if g_rec >= big_n {
                continue;
            }
            let g_flood = big_n - g_rec;
            let n = (g_flood / per_gas).max(1);
            // flood-only reference at the SAME victim count -> isolates the stack term by differencing.
            let f_only = run_vm_flood_n(
                n,
                s,
                (n * per_gas + 300_000).min(u32::MAX as u64) as u32,
                base,
            );
            let part_a = run_decommit_flood(n, s, base); // program_cache alone
            let comb = run_stack_then_flood(
                n,
                s,
                base,
                g_rec as u32,
                g_flood as u32,
                driver_gas(big_n),
                write_on_unwind,
            );
            let mass = (n as usize * s) as f64;
            let stack_nat = (comb.peak as f64 - f_only.peak as f64).max(0.0);
            let flood_nat = f_only.peak as f64;
            let (fp_rv32, _) = fp_rv32_of(mass, part_a.peak as f64, flood_nat, stack_nat);
            let fp_rv32_mib = mb(fp_rv32);
            let tot = fp_rv32_mib + b_min_rv32;
            let v = |cap: f64| {
                if tot < cap {
                    format!("PASS +{:.0}", cap - tot)
                } else {
                    format!("FAIL -{:.0}", tot - cap)
                }
            };
            println!(
                "{:>8.1} {:>8.1} {:>5} | {:>9.0} {:>9.0} {:>9.0} | {:>9.0} | {:>13} {:>13}",
                g_rec as f64 / 1e6,
                g_flood as f64 / 1e6,
                n,
                mb(stack_nat),
                mb(flood_nat),
                fp_rv32_mib,
                tot,
                v(768.0),
                v(950.0)
            );
            println!("      lfd comb={} vs flood-only={} (must match, else the differenced stack term is wrong) | {} | {}", comb.lfd, f_only.lfd, comb.extra, f_only.extra);
            assert_eq!(
                comb.lfd, f_only.lfd,
                "combined run did not decommit the same victim count as the flood-only reference"
            );
            if best.map_or(true, |(b, _)| tot > b) {
                best = Some((tot, g_rec));
            }
        }
        if let Some((tot, g_rec)) = best {
            println!(
                "  => Case A worst split: g_rec={:.1}M -> tot_rv32={tot:.0} MiB (all-flood is g_rec=0)",
                g_rec as f64 / 1e6
            );
        }

        // ---- Case B: TWO txs. Each phase gets its own full N -> no gas competition. ----
        println!("\n[Case B] 2 txs: tx1 = full N on stack inflation, tx2 = full N on flood (pool persists across txs)");
        let n = (big_n / per_gas).max(1);
        let f_only = run_vm_flood_n(
            n,
            s,
            (n * per_gas + 300_000).min(u32::MAX as u64) as u32,
            base,
        );
        let part_a = run_decommit_flood(n, s, base);
        let comb = run_stack_then_flood(
            n,
            s,
            base,
            big_n as u32,
            big_n as u32,
            driver_gas(2 * big_n),
            write_on_unwind,
        );
        let mass = (n as usize * s) as f64;
        let stack_nat = (comb.peak as f64 - f_only.peak as f64).max(0.0);
        let flood_nat = f_only.peak as f64;
        let (fp_rv32, heap_nat) = fp_rv32_of(mass, part_a.peak as f64, flood_nat, stack_nat);
        let _ = heap_nat;
        let fp_rv32_mib = mb(fp_rv32);
        let tot = fp_rv32_mib + b_min_rv32;
        let v = |cap: f64| {
            if tot < cap {
                format!("PASS +{:.0}", cap - tot)
            } else {
                format!("FAIL -{:.0}", tot - cap)
            }
        };
        println!(
            "  N_vic={n} stack_nat={:.0} flood_nat={:.0} fp_rv32={:.0} tot_rv32={:.0} | 768: {} | 950: {}",
            mb(stack_nat),
            mb(flood_nat),
            fp_rv32_mib,
            tot,
            v(768.0),
            v(950.0)
        );
        println!(
            "      lfd comb={} vs flood-only={} | {} | {}",
            comb.lfd, f_only.lfd, comb.extra, f_only.extra
        );
        assert_eq!(
            comb.lfd, f_only.lfd,
            "combined run did not decommit the same victim count as the flood-only reference"
        );
        // The pool must actually have been built, else the phases mis-sequenced (see `run_stack_then_flood`).
        assert!(
            mb(stack_nat) > 40.0,
            "stack term {:.1} MiB too small: phase 1 likely never materialized the pool",
            mb(stack_nat)
        );
    }
    println!(
        "\nstack term is ~vm2-arm-independent: the flood re-pops only the TOP pool slot, so #115"
    );
    println!("does not reclaim the deep pool either (measured delta #115 vs #124 ~= 4 MiB).");
}

/// Finding 3 — **an inflated stack pool survives a rollback**, so the transaction that inflates it
/// does not have to succeed.
///
/// `stack_pool` is a field of `VirtualMachine`, while `VmSnapshot` carries only `world_snapshot` +
/// `state_snapshot`, and `State::snapshot` does not include the pool. Reading the source says the
/// pool cannot be rolled back; this executes it end-to-end through the public snapshot API, which is
/// exactly the sequence the bootloader performs around a transaction it may revert:
/// `make_snapshot` -> tx inflates the pool -> `rollback`.
///
/// Note `rollback` does deallocate non-pinned heaps, so residency legitimately dips; the assertion
/// is that the stack-pool-sized bulk is still resident afterwards.
#[test]
#[ignore = "node-free stack-pool rollback survival check; run with --ignored"]
fn mem_dos_stack_pool_survives_rollback() {
    let g_rec: u32 = std::env::var("MEM_DOS_GAS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000_000);
    let write_on_unwind = std::env::var("MEM_DOS_WRITE_ORDER").map_or(true, |v| v != "in");
    let s = 4096usize; // tiny victims: this test is about the stack pool, not the flood
    let base = 0x10000u64;
    let words = s / 32;

    let mut storage = build_storage(1, s, base);
    let mat_info = code_info(MAT_ID, words);
    storage.code_keys.insert(
        code_info_key(U256::from_big_endian(MAT.as_bytes())),
        u256_to_h256(mat_info),
    );
    let mut pinned = HashMap::new();
    pinned.insert(mat_info, materializer(write_on_unwind));
    pinned.insert(U256::from(AA_HASH), aa_program());
    let mut world: W = World::new(storage, pinned);

    let baseline = LIVE.load(Ordering::Relaxed);
    let mut vm = VirtualMachine::new(
        DRIVER,
        driver(g_rec, 0), // phase 2 gets no gas: inflate the pool only
        Address::zero(),
        &[],
        driver_gas(g_rec as u64),
        Settings {
            default_aa_code_hash: u256_to_h256(U256::from(AA_HASH)).to_fixed_bytes(),
            evm_interpreter_code_hash: [0; 32],
            hook_address: 0,
        },
    );

    // The bootloader's pre-transaction snapshot.
    vm.make_snapshot();
    let end = vm.run(&mut world, &mut ());
    let after_run = LIVE.load(Ordering::Relaxed).saturating_sub(baseline);

    // The transaction reverts and the bootloader rolls the VM back.
    vm.rollback();
    let after_rollback = LIVE.load(Ordering::Relaxed).saturating_sub(baseline);

    println!(
        "ROLLBACK end={end:?} write_on_unwind={write_on_unwind} gas={g_rec}\n  \
         resident after run      = {:.1} MiB\n  \
         resident after rollback = {:.1} MiB  ({:.1}% retained)",
        mb(after_run as f64),
        mb(after_rollback as f64),
        100.0 * after_rollback as f64 / (after_run as f64).max(1.0)
    );

    assert!(
        mb(after_run as f64) > 100.0,
        "phase 1 did not inflate the pool ({:.1} MiB)",
        mb(after_run as f64)
    );
    assert!(
        after_rollback * 10 >= after_run * 9,
        "expected the stack pool to survive rollback, but residency fell from {:.1} to {:.1} MiB",
        mb(after_run as f64),
        mb(after_rollback as f64)
    );
    println!("  => the pool is NOT undone by rollback: a REVERTED tx still leaves it resident.");
}
