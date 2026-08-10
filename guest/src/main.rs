#![no_main]

#[cfg(not(feature = "decode-bench"))]
use airbender::guest::read;
#[cfg(not(feature = "decode-bench"))]
use zksync_airbender_verifier::types::AirbenderVerifierInput;
#[cfg(not(feature = "decode-bench"))]
use zksync_airbender_verifier::Verify;

#[cfg(not(feature = "decode-bench"))]
#[airbender::main]
fn main() -> [u32; 8] {
    let input: AirbenderVerifierInput = read().expect("failed to read AirbenderVerifierInput");
    let result = input.verify().unwrap();
    result.proof_public_input
}

// ---------------------------------------------------------------------------
// decode-bench: measures REAL guest cycles for `zksync_vm2::Program::new`, the
// per-far-call decode that PR#92's bounded cache forces to re-run on every
// eviction miss. NOT a shippable guest (no soundness): a calibration harness
// only, gated behind the `decode-bench` feature. See the DoS analysis.
#[cfg(feature = "decode-bench")]
mod bench {
    use airbender::guest::read;
    use primitive_types::{H160, U256};
    use serde::{Deserialize, Serialize};
    use zksync_vm2::{Program, StorageSlot};

    /// Config pushed by the host runner (identical layout on both sides).
    #[derive(Serialize, Deserialize)]
    pub struct BenchCfg {
        /// Number of full `Program::new` decodes to perform in the timed loop.
        pub k: u32,
        /// Bytecode size in 32-byte words (max on-chain = 65_535, odd).
        pub size_words: u32,
        /// If non-zero, also perform a full `Vec<u8>` clone per iteration —
        /// models PR#93's per-miss `load_factory_dep` bytecode clone.
        pub do_clone: u8,
    }

    /// Minimal `World` stub. `Program::new` only needs `W: World<T>` as a type
    /// bound to construct `Instruction<T, W>` handler pointers; none of these
    /// methods are ever called during decode, so they may all be unreachable.
    struct BenchWorld;
    impl zksync_vm2::StorageInterface for BenchWorld {
        fn read_storage(&mut self, _: H160, _: U256) -> StorageSlot {
            unreachable!()
        }
        fn cost_of_writing_storage(&mut self, _: StorageSlot, _: U256) -> u32 {
            unreachable!()
        }
        fn is_free_storage_slot(&self, _: &H160, _: &U256) -> bool {
            unreachable!()
        }
    }
    impl zksync_vm2::World<()> for BenchWorld {
        fn decommit(&mut self, _: U256) -> Program<(), Self> {
            unreachable!()
        }
        fn decommit_code(&mut self, _: U256) -> Vec<u8> {
            unreachable!()
        }
    }

    #[airbender::main]
    fn main() -> [u32; 8] {
        let cfg: BenchCfg = read().expect("failed to read BenchCfg");

        // Build the source bytecode ONCE, outside the timed loop, so the timed
        // work is purely `Program::new` (+ optional clone). Any content decodes
        // identically (decode is content-independent), so a cheap fill is fine.
        let len = cfg.size_words as usize * 32;
        let code: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(7)).collect();

        let mut acc: u64 = 0;
        for _ in 0..cfg.k {
            let program: Program<(), BenchWorld> = Program::new(&code, false);
            // Touch the decoded output so the decode cannot be optimized away.
            let cp = program.code_page();
            if !cp.is_empty() {
                acc = acc.wrapping_add(cp[0].low_u64());
            }
            acc = acc.wrapping_add(core::hint::black_box(program.code_page().len()) as u64);
            if cfg.do_clone != 0 {
                let c = code.clone();
                acc = acc.wrapping_add(core::hint::black_box(c[0]) as u64);
            }
        }

        let lo = acc as u32;
        let hi = (acc >> 32) as u32;
        [lo, hi, cfg.k, cfg.size_words, cfg.do_clone as u32, 0, 0, 0]
    }
}
