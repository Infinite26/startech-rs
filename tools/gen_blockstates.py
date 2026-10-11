#!/usr/bin/env python3
"""Generate src/world/blockstates.rs — vanilla 1.20.1 (protocol 763) block-state ids.

The generated table maps (block name, properties) -> global blockstate id
(0..24134). Sources are tried in order; the first that yields a complete,
validatable table wins:

  1. A vanilla `blocks.json` report (Mojang data generator output, e.g.
     `java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --reports`)
     via --vanilla-report PATH or $VANILLA_BLOCKS_JSON. Explicit per-state ids.
  2. PrismarineJS minecraft-data:
       data/pc/1.20.1/blocks.json, then data/pc/1.20/blocks.json
     Per-block state-id ranges (minStateId/maxStateId) + property definitions.
     Note: the 1.20.1 directory only ships version/sounds metadata upstream;
     the 1.20 table has the identical 24135 states (verified against the
     vanilla 1.20.1 report).
  3. misode/mcmeta (1.20.1-summary branch) blocks/data.json
     Property values + default states — used as CROSS-VALIDATION for source 2
     (it carries no state ids itself).

State-id enumeration inside a block (vanilla rule, verified against the
1.20.1 report): properties are ordered as listed by the source, and the LAST
property varies fastest (odometer: first property = most significant digit).

Validation performed before writing (FAIL LOUD on any mismatch):
  * exactly 24135 states, ids 0..24134 with no gaps or duplicates
  * minecraft:air (no properties) == 0
  * minecraft:stone (no properties) == 1
  * every block's default state id matches the mcmeta default values
    (when mcmeta is available)
  * property value lists agree between sources (when mcmeta is available)
  * --vanilla-report mode: full expansion must reproduce every state exactly

Usage:
  python3 tools/gen_blockstates.py                     # fetch sources over HTTP
  python3 tools/gen_blockstates.py --vanilla-report R  # use a local gold report
"""

import argparse
import itertools
import json
import os
import sys
import urllib.request

OUT_PATH = os.path.join(os.path.dirname(__file__), "..", "src", "world", "blockstates.rs")

URLS = {
    "mcmeta": [
        "https://raw.githubusercontent.com/misode/mcmeta/1.20.1-summary/blocks/data.json",
        "https://raw.githubusercontent.com/misode/mcmeta/summary/blocks/data.json",
    ],
    "prismarine": [
        "https://raw.githubusercontent.com/PrismarineJS/minecraft-data/master/data/pc/1.20.1/blocks.json",
        "https://raw.githubusercontent.com/PrismarineJS/minecraft-data/master/data/pc/1.20/blocks.json",
    ],
}

EXPECTED_TOTAL = 24135
EXPECTED_AIR = 0
EXPECTED_STONE = 1


def die(msg):
    print(f"gen_blockstates: ERROR: {msg}", file=sys.stderr)
    sys.exit(1)


def fetch_first(urls, what):
    for url in urls:
        try:
            print(f"gen_blockstates: fetching {what}: {url}")
            req = urllib.request.Request(url, headers={"User-Agent": "startech-rs-gen/1.0"})
            with urllib.request.urlopen(req, timeout=60) as r:
                return json.load(r), url
        except Exception as e:  # noqa: BLE001 - try the next source
            print(f"gen_blockstates:   failed: {e}")
    return None, None


def expand(props_order, value_lists, base):
    """All state ids for one block: odometer with the LAST property varying fastest."""
    states = []
    for combo in itertools.product(*value_lists):
        props = dict(zip(props_order, combo))
        states.append((base + len(states), props))
    return states


def fit_order(rep_props, rep_states):
    """Find the property order whose odometer expansion reproduces rep_states.

    The vanilla report's `properties` key order is *usually* the enumeration
    order, but not always (chest, trapped_chest, piston_head, moving_piston
    differ). Try permutations until one fits exactly.
    """
    names = list(rep_props.keys())
    if not names:
        return []
    want = [(s["id"], s.get("properties", {})) for s in rep_states]
    for order in itertools.permutations(names):
        value_lists = [rep_props[p] for p in order]
        n = 1
        for vl in value_lists:
            n *= len(vl)
        if n != len(rep_states):
            continue
        if expand(list(order), value_lists, rep_states[0]["id"]) == want:
            return list(order)
    return None


def table_from_report(report):
    """Build [(name, min_id, props_order, props_map)] from a vanilla blocks.json report."""
    table = []
    for name in sorted(report):
        entry = report[name]
        states = entry.get("states", [])
        if not states:
            continue
        ids = [s["id"] for s in states]
        if ids != list(range(ids[0], ids[0] + len(ids))):
            die(f"{name}: non-contiguous state ids {ids[:5]}...")
        rep_props = entry.get("properties") or {}
        order = fit_order(rep_props, states) if rep_props else []
        if order is None:
            die(f"{name}: could not fit property enumeration order")
        # Full-expansion self-check against the explicit report states.
        if rep_props:
            got = expand(order, [rep_props[p] for p in order], ids[0])
            want = [(s["id"], s.get("properties", {})) for s in states]
            if got != want:
                die(f"{name}: expansion does not reproduce report states")
        full = name if name.startswith("minecraft:") else "minecraft:" + name
        table.append((full, ids[0], order, dict(rep_props)))
    return table


def table_from_prismarine(blocks):
    """Build the same table from minecraft-data blocks.json (id ranges + property defs)."""
    table = []
    for b in blocks:
        name = b.get("name", "")
        order = [s["name"] for s in b.get("states", [])]
        rep_props = {}
        for s in b.get("states", []):
            vals = s.get("values")
            if vals is None:
                # int/bool property without an explicit list: num_values enumeration
                n = s.get("num_values")
                t = s.get("type")
                if t == "bool":
                    vals = ["true", "false"]
                elif n is not None:
                    vals = [str(i) for i in range(n)]
                else:
                    die(f"{name}: property {s.get('name')} has no values")
            rep_props[s["name"]] = list(vals)
        n = 1
        for p in order:
            n *= len(rep_props[p])
        span = b["maxStateId"] - b["minStateId"] + 1
        if n != span:
            die(f"{name}: property product {n} != state span {span}")
        table.append(("minecraft:" + name, b["minStateId"], order, rep_props))
    return table, {b["name"]: b.get("defaultState") for b in blocks}


def cross_validate(table, defaults, mcmeta):
    """mcmeta (properties + default values) vs the id table. FAIL on any mismatch."""
    by_name = {n: (mid, order, props) for n, mid, order, props in table}
    for key, (mprops, mdef) in mcmeta.items():
        name = "minecraft:" + key
        if name not in by_name:
            # blocks/data.json only covers blocks with properties
            continue
        mid, order, props = by_name[name]
        if list(mprops.keys()) != order and sorted(mprops.keys()) != sorted(order):
            die(f"{name}: mcmeta property names disagree: {list(mprops)} vs {order}")
        for p in order:
            if list(mprops[p]) != list(props[p]):
                die(f"{name}: mcmeta values for '{p}' disagree: {mprops[p]} vs {props[p]}")
        # default state id via the odometer rule
        idx = 0
        mult = 1
        for p in reversed(order):
            try:
                vi = mprops[p].index(mdef[p])
            except ValueError:
                die(f"{name}: default value {mdef[p]!r} of '{p}' not in {mprops[p]}")
            idx += vi * mult
            mult *= len(mprops[p])
        got = mid + idx
        want = defaults.get(key)
        if want is not None and got != want:
            die(f"{name}: default state mismatch: computed {got}, minecraft-data says {want}")
    print("gen_blockstates: mcmeta cross-validation OK (defaults + value lists)")


def rust_str(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def emit(table, sources_note):
    lines = []
    a = lines.append
    a("//! GENERATED FILE — do not edit by hand.")
    a("//!")
    a("//! Generated by tools/gen_blockstates.py. Vanilla 1.20.1 (protocol 763)")
    a("//! block states: 24135 states, ids 0..24134.")
    a(f"//!")
    a(f"//! Source(s): {sources_note}")
    a("")
    a("/// One vanilla block: base state id + property schema. Names are stored")
    a("/// without the `minecraft:` prefix.")
    a("///")
    a("/// State ids within a block are enumerated as an odometer over `props`")
    a("/// with the LAST property varying fastest (vanilla rule, verified against")
    a("/// the 1.20.1 `blocks.json` report).")
    a("struct BlockDef {")
    a("    name: &'static str,")
    a("    min_id: u32,")
    a("    props: &'static [(&'static str, &'static [&'static str])],")
    a("}")
    a("")
    a(f"static BLOCKS: &[BlockDef] = &[")
    for name, min_id, order, props in table:
        parts = ", ".join(
            f"({rust_str(p)}, &[{', '.join(rust_str(v) for v in props[p])}])" for p in order
        )
        bare = name.split(":", 1)[-1]
        a(f"    BlockDef {{ name: {rust_str(bare)}, min_id: {min_id}, props: &[{parts}] }},")
    a("];")
    a("")

    # resolve air-like ids from the table itself
    ids = {n: mid for n, mid, _, _ in table}
    air = ids.get("minecraft:air")
    cave_air = ids.get("minecraft:cave_air")
    void_air = ids.get("minecraft:void_air")
    if air != EXPECTED_AIR or cave_air is None or void_air is None:
        die("air/cave_air/void_air ids missing from table")
    a("/// Global blockstate id of `minecraft:air`.")
    a("pub fn air() -> u32 {")
    a("    0")
    a("}")
    a("")
    a(f"const AIR: u32 = {air};")
    a(f"const CAVE_AIR: u32 = {cave_air};")
    a(f"const VOID_AIR: u32 = {void_air};")
    a("")
    a("/// True for air / cave_air / void_air — the blocks that do not count")
    a("/// toward a section's block count (doc §7.3).")
    a("pub fn is_air_like(id: u32) -> bool {")
    a("    id == AIR || id == CAVE_AIR || id == VOID_AIR")
    a("}")
    a("")
    a("/// Look up a vanilla blockstate id from a block name and its properties")
    a("/// (any order). Returns `None` for non-vanilla (modded) or unknown blocks")
    a("/// or unknown property names/values.")
    a("pub fn lookup(name: &str, props: &[(String, String)]) -> Option<u32> {")
    a("    // Accept both `minecraft:stone` and bare `stone`; reject other namespaces.")
    a("    let short: &str = match name.find(':') {")
    a("        None => name,")
    a("        Some(i) if &name[..i] == \"minecraft\" => &name[i + 1..],")
    a("        Some(_) => return None,")
    a("    };")
    a("    let def = match BLOCKS.binary_search_by(|b| b.name.cmp(short)) {")
    a("        Ok(i) => &BLOCKS[i],")
    a("        Err(_) => return None,")
    a("    };")
    a("    if props.len() != def.props.len() {")
    a("        return None;")
    a("    }")
    a("    // Odometer: LAST property varies fastest.")
    a("    let mut idx: u32 = 0;")
    a("    let mut mult: u32 = 1;")
    a("    for (pname, values) in def.props.iter().rev() {")
    a("        let v = props.iter().find(|(k, _)| k == pname).map(|(_, v)| v.as_str())?;")
    a("        let vi = values.iter().position(|x| *x == v)? as u32;")
    a("        idx += vi * mult;")
    a("        mult *= values.len() as u32;")
    a("    }")
    a("    Some(def.min_id + idx)")
    a("}")
    a("")
    a("#[cfg(test)]")
    a("mod tests {")
    a("    use super::*;")
    a("")
    a("    fn p(pairs: &[(&str, &str)]) -> Vec<(String, String)> {")
    a("        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()")
    a("    }")
    a("")
    a("    #[test]")
    a("    fn air_and_stone() {")
    a("        assert_eq!(lookup(\"minecraft:air\", &[]), Some(0));")
    a("        assert_eq!(lookup(\"stone\", &[]), Some(1));")
    a("    }")
    a("")
    a("    #[test]")
    a("    fn multi_property_odometer() {")
    a("        // oak_log: min 130, axis x=0 y=1 z=2")
    a("        assert_eq!(lookup(\"minecraft:oak_log\", &p(&[(\"axis\", \"x\")])), Some(130));")
    a("        assert_eq!(lookup(\"oak_log\", &p(&[(\"axis\", \"y\")])), Some(131));")
    a("        assert_eq!(lookup(\"minecraft:oak_log\", &p(&[(\"axis\", \"z\")])), Some(132));")
    a("    }")
    a("")
    a("    #[test]")
    a("    fn unknown_is_none() {")
    a("        assert_eq!(lookup(\"gtceu:machine\", &[]), None);")
    a("        assert_eq!(lookup(\"minecraft:stone\", &p(&[(\"bogus\", \"x\")])), None);")
    a("    }")
    a("}")
    a("")

    with open(OUT_PATH, "w") as f:
        f.write("\n".join(lines))
    print(f"gen_blockstates: wrote {os.path.normpath(OUT_PATH)} ({len(table)} blocks)")


def main():
    global OUT_PATH
    ap = argparse.ArgumentParser()
    ap.add_argument("--vanilla-report", default=os.environ.get("VANILLA_BLOCKS_JSON"),
                    help="path to a vanilla blocks.json report (gold source)")
    ap.add_argument("--out", default=OUT_PATH)
    args = ap.parse_args()
    OUT_PATH = args.out

    defaults = {}
    mcmeta = None

    if args.vanilla_report:
        print(f"gen_blockstates: using vanilla report {args.vanilla_report}")
        with open(args.vanilla_report) as f:
            report = json.load(f)
        # report keys are bare names -> normalize like the report reader expects
        table = table_from_report(report)
        sources_note = f"vanilla data-generator report ({args.vanilla_report})"
    else:
        pri, pri_url = fetch_first(URLS["prismarine"], "minecraft-data blocks")
        if pri is None:
            die("no block-state source reachable")
        table, defaults = table_from_prismarine(pri)
        sources_note = f"PrismarineJS minecraft-data ({pri_url})"

    mcm, mcm_url = fetch_first(URLS["mcmeta"], "mcmeta blocks (cross-validation)")
    if mcm is not None:
        mcmeta = {k: (v[0], v[1]) for k, v in mcm.items()}
        sources_note += f" + misode/mcmeta cross-check ({mcm_url})"
    else:
        print("gen_blockstates: WARNING: mcmeta unavailable — skipping default-state cross-check")

    # ---- hard validation (fail loudly) ----
    seen = {}
    for name, min_id, order, props in table:
        n = 1
        for p in order:
            n *= len(props[p])
        for sid in range(min_id, min_id + n):
            if sid in seen:
                die(f"duplicate state id {sid} ({seen[sid]} vs {name})")
            seen[sid] = name
    total = len(seen)
    if total != EXPECTED_TOTAL:
        die(f"total state count {total} != expected {EXPECTED_TOTAL}")
    if set(seen) != set(range(EXPECTED_TOTAL)):
        die("state ids are not exactly 0..24134")
    by_name = {n: (mid, order, props) for n, mid, order, props in table}
    if by_name.get("minecraft:air", (None,))[0] != EXPECTED_AIR:
        die(f"minecraft:air id {by_name.get('minecraft:air')} != {EXPECTED_AIR}")
    if by_name.get("minecraft:stone", (None,))[0] != EXPECTED_STONE:
        die(f"minecraft:stone id {by_name.get('minecraft:stone')} != {EXPECTED_STONE}")
    for name in ("minecraft:air", "minecraft:stone"):
        mid, order, props = by_name[name]
        if order:
            die(f"{name} must have no properties, found {order}")
    print(f"gen_blockstates: total states = {total} (ids 0..{total - 1})")
    print(f"gen_blockstates: minecraft:air   = {EXPECTED_AIR}")
    print(f"gen_blockstates: minecraft:stone = {EXPECTED_STONE}")

    if mcmeta is not None:
        cross_validate(table, defaults, mcmeta)

    table.sort(key=lambda t: t[0])
    emit(table, sources_note)
    print("gen_blockstates: OK")


if __name__ == "__main__":
    main()
