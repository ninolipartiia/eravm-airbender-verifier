//! Runs the `decode-bench` guest (guest built with `--features decode-bench`)
//! through the transpiler and prints ground-truth `cycles_executed` for `k`
//! full `zksync_vm2::Program::new` decodes of a `size_words`-word bytecode.
//!
//! Two runs at different `k` (same `size_words`) let you subtract the fixed
//! setup and isolate the per-decode guest-cycle cost — the measured factor in
//! the PR#92 re-decode cycle-DoS calculation.
//!
//! ```text
//! cargo run --release -p zksync_cycle_model --example decode_bench -- \
//!     --app-bin-dir artifacts/decode-bench/guest --k 128 --size-words 65535
//! ```

use std::path::PathBuf;

use airbender_host::{Inputs, Runner, TranspilerRunnerBuilder};
use anyhow::{Context, Result};
use clap::Parser;
use serde::Serialize;

/// Must match `bench::BenchCfg` in `guest/src/main.rs` field-for-field.
#[derive(Serialize)]
struct BenchCfg {
    k: u32,
    size_words: u32,
    do_clone: u8,
}

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "artifacts/decode-bench/guest")]
    app_bin_dir: PathBuf,
    /// Number of `Program::new` decodes in the timed loop.
    #[arg(long)]
    k: u32,
    /// Bytecode size in 32-byte words (max on-chain = 65535).
    #[arg(long)]
    size_words: u32,
    /// 1 = also clone the source bytecode each iter (models PR#93's per-miss clone).
    #[arg(long, default_value_t = 0)]
    do_clone: u8,
}

fn main() -> Result<()> {
    let a = Args::parse();
    let runner = TranspilerRunnerBuilder::new(a.app_bin_dir.join("app.bin"))
        .with_cycles(usize::MAX)
        .build()
        .context("building transpiler runner")?;

    let mut words = Inputs::new();
    words
        .push(&BenchCfg {
            k: a.k,
            size_words: a.size_words,
            do_clone: a.do_clone,
        })
        .context("encoding BenchCfg")?;

    let execution = runner.run(words.words()).context("guest run failed")?;
    println!(
        "k={} size_words={} do_clone={} cycles_executed={}",
        a.k, a.size_words, a.do_clone, execution.cycles_executed
    );
    Ok(())
}
