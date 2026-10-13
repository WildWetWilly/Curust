#!/usr/bin/env python3
"""Compare oracle output (real Cura) with the Rust settings port (oracle_compare example).

Usage: compare.py oracle.jsonl rust.jsonl [-v]
"""
import json, sys

def load(p):
    return {r["name"]: r for r in (json.loads(l) for l in open(p) if l.strip())}

def main():
    a, b = load(sys.argv[1]), load(sys.argv[2])
    verbose = "-v" in sys.argv
    total_bad = 0
    for name, ra in a.items():
        rb = b.get(name)
        problems = []
        if rb is None:
            print(f"{name}: missing in rust output"); total_bad += 1; continue
        if "error" in ra or "error" in rb:
            print(f"{name}: error oracle={ra.get('error','')[-300:]!r} rust={rb.get('error','')!r}")
            total_bad += 1
            continue
        import re as _re
        norm = lambda c: json.loads(_re.sub(r"oracle_\d+", "oracle_N", json.dumps(c)))
        if norm(ra["containers"]) != norm(rb["containers"]):
            problems.append(f"containers differ:\n   oracle {ra['containers']}\n   rust   {rb['containers']}")
        ga, gb = dict(ra["global_settings"]), dict(rb["global_settings"])
        for k in sorted(set(ga) | set(gb)):
            if ga.get(k) != gb.get(k):
                problems.append(f"global {k}: oracle={ga.get(k)!r} rust={gb.get(k)!r}")
        la, lb = sorted(map(tuple, ra["limit_to_extruder"])), sorted(map(tuple, rb["limit_to_extruder"]))
        if la != lb:
            problems.append(f"limit_to_extruder: only oracle {sorted(set(la)-set(lb))} only rust {sorted(set(lb)-set(la))}")
        ea = {e["id"]: dict(e["settings"]) for e in ra["extruders"]}
        eb = {e["id"]: dict(e["settings"]) for e in rb["extruders"]}
        if sorted(ea) != sorted(eb):
            problems.append(f"extruder ids differ {sorted(ea)} vs {sorted(eb)}")
        for i in ea:
            if i not in eb: continue
            for k in sorted(set(ea[i]) | set(eb[i])):
                if ea[i].get(k) != eb[i].get(k):
                    problems.append(f"extruder {i} {k}: oracle={ea[i].get(k)!r} rust={eb[i].get(k)!r}")
        oa = [dict(o["settings"]) for g in ra.get("objects", []) for o in g["objects"]]
        ob = [dict(o) for o in rb.get("object_settings", [])]
        if len(oa) != len(ob):
            problems.append(f"object count {len(oa)} vs {len(ob)}")
        for n, (x, y) in enumerate(zip(oa, ob)):
            for k in sorted(set(x) | set(y)):
                if x.get(k) != y.get(k):
                    problems.append(f"object {n} {k}: oracle={x.get(k)!r} rust={y.get(k)!r}")
        if problems:
            total_bad += 1
            print(f"{name}: {len(problems)} differences")
            for p in problems[: (10000 if verbose else 12)]:
                print("   ", p)
        else:
            print(f"{name}: OK")
    print(f"{len(a) - total_bad}/{len(a)} jobs identical")

main()
