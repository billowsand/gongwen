#!/usr/bin/env python3
"""Generic single-file splitter; configuration is provided as argv[1] (a python file
defining SRC, OUT_DIR, MODULE, TARGETS, ROOT_ITEMS, RE_EXPORTS, MODS, DOCS, ...)."""
import os
import re
import sys

exec(compile(open(sys.argv[1], encoding="utf-8").read(), sys.argv[1], "exec"))

SUPER_MAP = SUPER_MAP if "SUPER_MAP" in dir() else {}
PER_FILE_EXTRA = PER_FILE_EXTRA if "PER_FILE_EXTRA" in dir() else {}
ROOT_PUB_UPGRADES = ROOT_PUB_UPGRADES if "ROOT_PUB_UPGRADES" in dir() else []
ROOT_PREAMBLE = ROOT_PREAMBLE if "ROOT_PREAMBLE" in dir() else []

# ============================== SPLITTER ==============================
with open(SRC, encoding="utf-8") as f:
    lines = f.read().split("\n")
N = len(lines)

BOUNDARY = re.compile(
    r"^(pub\(crate\) )?(pub )?(struct|enum|impl|fn|mod|const|type) |^    (pub\(crate\) )?(pub )?fn "
)
boundaries = [i for i, l in enumerate(lines) if BOUNDARY.match(l)]
assert boundaries == sorted(boundaries)

for start_1b in TARGETS:
    if start_1b - 1 not in boundaries:
        print(f"ERROR: target line {start_1b} is not a detected boundary")
        sys.exit(1)


# string state: (kind, quote) where kind is -1 normal, 0..n raw string with n hashes
def scan_line_braces(line, state):
    """Scan one line, updating brace depth and cross-line string state."""
    in_str, quote, raw = state
    depth = 0
    opened = False
    j = 0
    while j < len(line):
        ch = line[j]
        if not in_str:
            if ch == 'r' and j + 1 < len(line) and line[j + 1] == '"':
                in_str = True
                quote = '"'
                raw = 0
                j += 1
            elif ch == 'r' and j + 1 < len(line) and line[j + 1] == '#':
                k = j + 1
                while k < len(line) and line[k] == '#':
                    k += 1
                if k < len(line) and line[k] == '"':
                    in_str = True
                    quote = '"'
                    raw = k - j - 1
                    j = k
            elif ch == '"':
                in_str = True
                quote = ch
                raw = -1
            elif ch == "'":
                if j + 2 < len(line) and line[j + 2] == "'" and line[j + 1] != "\\":
                    j += 2
                elif j + 3 < len(line) and line[j + 1] == "\\" and line[j + 3] == "'":
                    j += 3
            elif ch == '/' and j + 1 < len(line) and line[j + 1] == '/':
                break
            elif ch == '{':
                depth += 1
                opened = True
            elif ch == '}':
                depth -= 1
        else:
            if raw == -1 and ch == '\\':
                j += 1
            elif ch == '"':
                if raw == 0:
                    in_str = False
                else:
                    # raw string r#"..."#: closing is `"` followed by raw hashes
                    h = j + 1
                    while h < len(line) and h < j + 1 + raw and line[h] == '#':
                        h += 1
                    if h == j + 1 + raw:
                        in_str = False
                        j = h - 1
        j += 1
    return (in_str, quote, raw), depth, opened


def find_item_end(start, next_start):
    depth = 0
    opened = False
    state = (False, None, -1)
    for i in range(start, min(next_start, N)):
        state, delta, opened = scan_line_braces(lines[i], state)
        depth += delta
        if opened and depth <= 0:
            return i
    return next_start - 1


def closing_brace_line(header_idx):
    depth = 0
    for ch in lines[header_idx]:
        if ch == '{':
            depth += 1
        elif ch == '}':
            depth -= 1
    state = (False, None, -1)
    i = header_idx + 1
    while i < N:
        state, delta, _ = scan_line_braces(lines[i], state)
        depth += delta
        if depth <= 0:
            return i
        i += 1
    return None


items = []
for idx, start in enumerate(boundaries):
    next_start = boundaries[idx + 1] if idx + 1 < len(boundaries) else N
    end = find_item_end(start, next_start)
    if end >= next_start:
        print(f"WARN: item at line {start + 1} end {end + 1} >= next start {next_start + 1}")
    items.append((start, end, TARGETS.get(start + 1)))

# move closing braces of fully-moved impl blocks along with them
for start, end, target in items:
    if target is None or target == "REMOVE":
        continue
    if not lines[start].lstrip().startswith("impl "):
        continue
    close = closing_brace_line(start)
    if close is None:
        continue
    last = None
    for idx, (s, e, t) in enumerate(items):
        if s > start and s < close:
            last = idx
    if last is None:
        for idx, (s, e, t) in enumerate(items):
            if s == start:
                items[idx] = (s, close, t)
    else:
        s, e, t = items[last]
        if t == target and e < close:
            items[last] = (s, close, t)


def comment_start(start):
    i = start - 1
    while i >= 0 and (lines[i].lstrip().startswith("///")
                      or lines[i].lstrip().startswith("//!")
                      or lines[i].lstrip().startswith("#[")):
        i -= 1
    return i + 1


files = {}
for start, end, target in items:
    if target == "REMOVE":
        continue
    c_start = comment_start(start)
    files.setdefault(target, []).append((c_start, start, end))

remove = set()
for start, end, target in items:
    if target == "REMOVE":
        remove.update(range(comment_start(start), end + 1))
    elif target is not None:
        c_start = comment_start(start)
        e = end
        while e > start and lines[e].lstrip().startswith("#["):
            e -= 1  # trailing attributes belong to the NEXT item; keep them in root
        remove.update(range(c_start, e + 1))
remove.update(REMOVE_LINES)
remove.update(HEADER_STRIP)
kept_lines = [l for i, l in enumerate(lines) if i not in remove]

# ------------------------------- import resolver -------------------------------
def word_re(name):
    return re.compile(r"\b" + re.escape(name) + r"\b")


def module_used(text, modname):
    return re.search(r"\b" + re.escape(modname) + r"::", text) is not None


def find_use_block(base):
    """Locate the `use {base}::{...};` block; return (start_idx, end_idx) or None."""
    for i, l in enumerate(lines):
        if l.startswith("use " + base + "::{"):
            depth = 0
            for j in range(i, min(i + 60, N)):
                depth += lines[j].count("{") - lines[j].count("}")
                if depth <= 0 and lines[j].strip().endswith("};"):
                    return i, j
    return None


def parse_use_block(text, base):
    body = text.strip()
    assert body.startswith("use " + base + "::{") and body.endswith("};"), body[:60]
    inner = body[len("use " + base + "::{"):-2]
    depth = 0
    seg = ""
    segs = []
    for ch in inner:
        if ch == '{':
            depth += 1
        elif ch == '}':
            depth -= 1
        if ch == ',' and depth == 0:
            segs.append(seg)
            seg = ""
        else:
            seg += ch
    if seg.strip():
        segs.append(seg)
    leaves = []
    for s in segs:
        s = s.strip()
        if not s:
            continue
        if '{' in s:
            head, _, rest = s.partition('{')
            head = head.strip().rstrip(':')
            rest = rest.rsplit('}', 1)[0]
            for name in [n.strip() for n in rest.split(',') if n.strip()]:
                leaves.append((base + "::" + head, name))
        elif "::" in s:
            m, _, n = s.rpartition("::")
            leaves.append((base + "::" + m, n))
        else:
            leaves.append((base, s))
    return leaves


def compute_imports(text, leaves):
    groups = {}
    for path, name in leaves:
        groups.setdefault(path, set()).add(name)
    out = []
    for path in sorted(groups):
        names = groups[path]
        if path in ("crate", "std"):
            for name in sorted(names):
                if name != "self" and (module_used(text, name) or word_re(name).search(text)):
                    out.append("use " + path + "::" + name + ";")
            continue
        needed = set()
        need_module = False
        for name in sorted(names):
            if name == "self":
                if module_used(text, path.rsplit("::", 1)[-1]):
                    need_module = True
            elif word_re(name.split(" as ")[-1]).search(text) \
                    or ("::" in name and word_re(name.split("::")[-1]).search(text)):
                needed.add(name)
            elif path.startswith("anyhow") and name == "Context" and re.search(r"\b(with_context|context)\(", text):
                needed.add(name)
        if not needed and not need_module:
            continue
        if needed:
            out.append("use " + path + "::{" + ", ".join(sorted(needed)) + "};")
        if need_module:
            out.append("use " + path + ";")
    return out


def block_leaves(base):
    blk = find_use_block(base)
    if blk is None:
        return []
    leaves = parse_use_block("\n".join(lines[blk[0]:blk[1] + 1]), base)
    mapped = SUPER_MAP.get(base)
    if mapped:
        leaves = [(mapped + p[len(base):], n) for p, n in leaves]
    return leaves

BLOCK_BASES = []
SINGLE_USES = {}
for _l in lines:
    _m = re.match(r"^use ([a-zA-Z0-9_:]+)::\{", _l)
    if _m and _m.group(1) not in BLOCK_BASES:
        BLOCK_BASES.append(_m.group(1))
    _s = re.match(r"^use ([a-zA-Z0-9_:]+)::([A-Za-z0-9_]+);$", _l)
    if _s and _s.group(1) not in BLOCK_BASES:
        BLOCK_BASES.append(_s.group(1))
        SINGLE_USES.setdefault(_s.group(1), []).append(_s.group(2))
BLOCK_LEAVES = {base: block_leaves(base) for base in BLOCK_BASES}
for _base, _names in SINGLE_USES.items():
    BLOCK_LEAVES.setdefault(_base, []).extend([(_base, _n) for _n in _names])


def _agg(base):
    leaves = []
    for _b, _ls in BLOCK_LEAVES.items():
        if _b == base or _b.startswith(base + "::"):
            leaves.extend(_ls)
    return leaves


def compute_crate_imports(text):
    return compute_imports(text, _agg("crate"))


def compute_std_imports(text):
    return compute_imports(text, _agg("std"))


def compute_egui_imports(text):
    return compute_imports(text, BLOCK_LEAVES.get("eframe::egui", []))


def compute_super_imports(text):
    return compute_imports(text, BLOCK_LEAVES.get("super", []))


def compute_anyhow_imports(text):
    return compute_imports(text, _agg("anyhow"))


DEF_RE = re.compile(r"^(pub\(crate\) )?(pub )?(struct|enum|fn|const|type) ([A-Za-z0-9_]+)", re.M)


def defined_names(text):
    return {m.group(4) for m in DEF_RE.finditer(text)}


def compute_root_imports(text):
    excluded = defined_names(text)
    needed = [n for n in ROOT_ITEMS if n not in excluded and word_re(n).search(text)]
    if not needed:
        return []
    return ["use crate::" + MODULE + "::{" + ", ".join(needed) + "};"]


def compute_extra_imports(text):
    out = []
    if "anyhow" not in BLOCK_LEAVES and (word_re("Context").search(text)
                                          or re.search(r"\b(with_context|context)\(", text)):
        out.append("use anyhow::Context;")
    if "anyhow" not in BLOCK_LEAVES and re.search(r"\bResult\b", text):
        out.append("use anyhow::Result;")
    if word_re("GenericImageView").search(text) or word_re("DynamicImage").search(text) \
            or re.search(r"\.dimensions\(", text):
        out.append("use image::GenericImageView;")
    if word_re("Regex").search(text):
        out.append("use regex::Regex;")
    cols = []
    if word_re("Column").search(text):
        cols.append("Column")
    if word_re("TableBuilder").search(text):
        cols.append("TableBuilder")
    if cols:
        out.append("use egui_extras::{" + ", ".join(cols) + "};")
    return out


TRAIT_IMPL = re.compile(r"^impl\s+[^{]*\s+for\s+")

def upgrade_visibility(name, content):
    for sig in PUB_UPGRADES.get(name, []):
        if sig in content:
            content = content.replace(sig, "pub(crate) " + sig, 1)
    out = []
    in_trait = False
    depth = 0
    for line in content.split("\n"):
        if not in_trait:
            m = TRAIT_IMPL.match(line)
            if m:
                in_trait = True
                depth = line.count("{") - line.count("}")
        else:
            depth += line.count("{") - line.count("}")
            if depth <= 0:
                in_trait = False
        if not in_trait:
            line = re.sub(r"^(pub\(crate\) )?(pub )?fn ", "pub(crate) fn ", line)
            line = re.sub(r"^    (pub\(crate\) )?(pub )?fn ", "    pub(crate) fn ", line)
        out.append(line)
    return "\n".join(out)


def is_impl_method(start):
    if not (IMPL_WRAP[0] <= start + 1 <= IMPL_WRAP[1]):
        return False
    return lines[start].startswith("    ")


# ---------------------------------- assembly ----------------------------------
def build_header(name, text):
    head = [
        "//! " + DOCS[name],
        "//!",
        "//! 由 " + SRC + " 拆分而来：本文件是模块 `" + MODULE + "::" + name + "`，与其它子模块共享",
        "//! `" + MODULE + "` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。",
        "",
    ]
    imports = []
    imports += compute_crate_imports(text)
    imports += compute_std_imports(text)
    imports += compute_super_imports(text)
    imports += compute_anyhow_imports(text)
    imports += compute_egui_imports(text)
    imports += compute_extra_imports(text)
    imports += compute_root_imports(text)
    imports += list(PER_FILE_EXTRA.get("*", []))
    imports += list(PER_FILE_EXTRA.get(name, []))
    imports += list(PER_FILE_EXTRA.get("root", []))
    _seen = set()
    imports = [i for i in imports if not (i in _seen or _seen.add(i))]
    if imports:
        head += imports
        head.append("")
    return "\n".join(head) + "\n"


os.makedirs(OUT_DIR, exist_ok=True)
for name in sorted(f for f in files if f):
    entries = files[name]
    normal = []
    methods = []
    for c_start, start, end in entries:
        chunk = lines[c_start:end + 1]
        while chunk and (not chunk[-1].strip() or chunk[-1].lstrip().startswith("///")
                         or chunk[-1].lstrip().startswith("#[")):
            chunk.pop()
        (methods if is_impl_method(start) else normal).append("\n".join(chunk))
    parts = normal
    if methods:
        parts.append(IMPL_WRAP_SIG + " {\n" + "\n\n".join(methods) + "\n}")
    content = "\n\n".join(parts) + "\n"
    content = upgrade_visibility(name, content)
    out_path = os.path.join(OUT_DIR, name + ".rs")
    with open(out_path, "w", encoding="utf-8") as f:
        f.write(build_header(name, content) + content)
    print("wrote %s: %d lines" % (out_path, len((build_header(name, content) + content).splitlines())))

# ------------------------------------ root ------------------------------------
kept_lines = [l for l in kept_lines if True]
for sig in ROOT_PUB_UPGRADES:
    for i, l in enumerate(kept_lines):
        if l == sig:
            kept_lines[i] = "pub(crate) " + l
kept_text = "\n".join(kept_lines)
mod_decls = "\n".join("mod " + m + ";" for m in MODS)
new_imports = []
new_imports += compute_crate_imports(kept_text)
new_imports += compute_std_imports(kept_text)
new_imports += compute_super_imports(kept_text)
new_imports += compute_anyhow_imports(kept_text)
new_imports += compute_egui_imports(kept_text)
new_imports += compute_extra_imports(kept_text)
new_imports += list(PER_FILE_EXTRA.get("root", []))

new_root = "\n".join(ROOT_DOC + [
    "",
] + ROOT_PREAMBLE + [
    "",
] + new_imports + [
    "",
    mod_decls,
    "",
    RE_EXPORTS,
    "",
    kept_text.strip(),
    "",
])
with open(SRC, "w", encoding="utf-8") as f:
    f.write(new_root)
print("wrote %s: %d lines" % (SRC, len(new_root.splitlines())))
print("items: %d total, %d moved, %d kept" % (
    len(items),
    sum(1 for _, _, t in items if t not in (None, "REMOVE")),
    sum(1 for _, _, t in items if t is None),
))
