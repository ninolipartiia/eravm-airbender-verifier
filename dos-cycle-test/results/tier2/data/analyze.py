#!/usr/bin/env python3
"""Turn sweep.csv (size_words,do_clone,k,cycles) into per-decode guest cycles
and the attack total vs the 2^36 prover ceiling."""
import csv, collections

CEIL = 2**36
rows = collections.defaultdict(dict)  # (size_words,do_clone) -> {k: cycles}
with open("artifacts/decode-bench/sweep.csv") as f:
    for r in csv.DictReader(f):
        rows[(int(r["size_words"]), int(r["do_clone"]))][int(r["k"])] = int(r["cycles"])

def per_decode(d):
    ks = sorted(d)
    k1, k2 = ks[0], ks[-1]
    return (d[k2] - d[k1]) / (k2 - k1), d

print(f"{'size_words':>10} {'bytes':>10} {'clone':>5} {'cyc/decode':>14} {'setup_cyc':>12}")
pd = {}
for (sw, clone), d in sorted(rows.items()):
    cpd, dd = per_decode(d)
    ks = sorted(dd)
    setup = dd[ks[0]] - cpd * ks[0]
    pd[(sw, clone)] = cpd
    print(f"{sw:>10} {sw*32:>10} {clone:>5} {cpd:>14,.0f} {setup:>12,.0f}")

# scaling: 2 MiB vs 512 KiB (both no-clone)
if (65535, 0) in pd and (16384, 0) in pd:
    print(f"\n2 MiB / 512 KiB decode ratio (RISC-V): {pd[(65535,0)]/pd[(16384,0)]:.2f}x "
          f"(native x86 bench measured 1.83x)")
# #93 clone add-on at 2 MiB
if (65535, 1) in pd and (65535, 0) in pd:
    add = pd[(65535,1)] - pd[(65535,0)]
    print(f"#93 per-miss clone add-on @2MiB: {add:,.0f} cyc "
          f"({100*add/pd[(65535,0)]:.1f}% of the decode)")

# combine with the analytic miss-count band
cpd_opt = pd.get((65535, 1), pd.get((65535, 0)))  # with #93 clone if present
print(f"\nprover ceiling 2^36 = {CEIL:,}")
print(f"using measured cyc/decode (2 MiB, +#93 clone) = {cpd_opt:,.0f}")
print(f"{'per-iter ergs':>13} {'misses':>10} {'total cycles':>16} {'x 2^36':>8}")
for per in (183, 200, 223, 250):
    misses = (80_000_000 - 64*1024*1024/8) / per
    total = misses * cpd_opt
    print(f"{per:>13} {misses:>10,.0f} {total:>16,.0f} {total/CEIL:>7.1f}x")
