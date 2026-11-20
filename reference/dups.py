import sys, time, pathlib, collections, hashlib
from tree_sitter_language_pack import get_parser, get_language
from tree_sitter import Query, QueryCursor
from datasketch import MinHash, MinHashLSH

be = pathlib.Path(sys.argv[1]); MIN_TOK = 60; MIN_LINES = 6; K = 4; TH = 0.8
p = get_parser("elixir")
q = Query(get_language("elixir"), '(call target: (identifier) @kw (#any-of? @kw "def" "defp" "defmacro" "defmacrop")) @fn')

def fn_name_node(clause):
    args = next((c for c in clause.named_children if c.type == "arguments"), None)
    if not args or not args.named_children: return None
    h = args.named_children[0]
    if h.type == "binary_operator" and h.named_children: h = h.named_children[0]
    if h.type == "call": return h.child_by_field_name("target")
    return h if h.type == "identifier" else None

def tokens(clause, src):
    name = fn_name_node(clause); name_id = name.id if name is not None else None
    keep = set(); l1 = []; l2 = []; stack = [clause]
    while stack:
        n = stack.pop()
        if n.type == "comment": continue
        if n.type == "call":
            t = n.child_by_field_name("target")
            if t is not None:
                if t.type == "identifier": keep.add(t.id)
                elif t.type == "dot":
                    r = t.child_by_field_name("right")
                    if r is not None: keep.add(r.id)
        if n.child_count == 0:
            txt = src[n.start_byte:n.end_byte].decode("utf-8", "ignore")
            if n.id == name_id: l1.append("FUNC"); l2.append("FUNC")
            elif n.type == "identifier": l1.append(txt); l2.append(txt if n.id in keep else "ID")
            elif n.type in ("integer", "float"): l1.append(txt); l2.append("NUM")
            elif n.type == "quoted_content": l1.append(txt); l2.append("STR")
            else: l1.append(txt); l2.append(txt)
        else:
            stack.extend(reversed(n.children))
    nm = src[name.start_byte:name.end_byte].decode() if name is not None else "?"
    return nm, l1, l2

def ctx(rel):
    parts = rel.parts
    return "/".join(parts[1:3]) if len(parts) > 3 else parts[1]

t0 = time.monotonic(); C = []; total = 0
for f in sorted((be / "lib").rglob("*.ex")):
    src = f.read_bytes(); rel = f.relative_to(be)
    for n in QueryCursor(q).captures(p.parse(src).root_node).get("fn", []):
        total += 1
        lines = n.end_point[0] - n.start_point[0] + 1
        if lines < MIN_LINES: continue
        nm, l1, l2 = tokens(n, src)
        if len(l2) < MIN_TOK: continue
        C.append(dict(path=str(rel), ctx=ctx(rel), s=n.start_point[0] + 1, e=n.end_point[0] + 1, name=nm, l1=l1, l2=l2))
print(f"clauses total={total} kept(>= {MIN_LINES} lines, >= {MIN_TOK} tokens)={len(C)} tokenise_s={time.monotonic()-t0:.1f}")

def groups(key):
    g = collections.defaultdict(list)
    for i, c in enumerate(C): g[hashlib.sha1("\x1f".join(c[key]).encode()).digest()].append(i)
    return [v for v in g.values() if len(v) > 1]
def describe(label, gs):
    files = [g for g in gs if len({C[i]["path"] for i in g}) > 1]
    ctxs = [g for g in gs if len({C[i]["ctx"] for i in g}) > 1]
    print(f"{label}: groups={len(gs)} clauses={sum(map(len, gs))} | cross-file groups={len(files)} clauses={sum(map(len, files))} | cross-context groups={len(ctxs)} clauses={sum(map(len, ctxs))}")
    return files, ctxs
g1 = groups("l1"); g2 = groups("l2")
describe("EXACT, only function name ignored", g1)
f2, c2 = describe("EXACT, variables/numbers/strings also ignored", g2)
def loc(i): c = C[i]; return f"{c['path']}:{c['s']}-{c['e']} {c['name']} ({len(c['l2'])} tok)"
print("\n-- largest cross-context exact groups (loose):")
for g in sorted(c2, key=lambda g: -len(C[g[0]]["l2"]))[:8]:
    print(f"  group of {len(g)}:"); [print("     ", loc(i)) for i in g[:4]]

t1 = time.monotonic(); sh = []; mh = []
for c in C:
    s = {" ".join(c["l2"][i:i + K]) for i in range(len(c["l2"]) - K + 1)}; sh.append(s)
    m = MinHash(num_perm=128); m.update_batch([x.encode() for x in s]); mh.append(m)
lsh = MinHashLSH(threshold=0.7, num_perm=128)
for i, m in enumerate(mh): lsh.insert(i, m)
exact_of = {}
for gi, g in enumerate(g2):
    for i in g: exact_of[i] = gi
pairs = []
for i, m in enumerate(mh):
    for j in lsh.query(m):
        if j <= i: continue
        if exact_of.get(i) is not None and exact_of.get(i) == exact_of.get(j): continue
        a, b = sh[i], sh[j]; jac = len(a & b) / len(a | b)
        if jac >= TH: pairs.append((jac, i, j))
xf = [x for x in pairs if C[x[1]]["path"] != C[x[2]]["path"]]
xc = [x for x in pairs if C[x[1]]["ctx"] != C[x[2]]["ctx"]]
same_fn = [x for x in pairs if C[x[1]]["path"] == C[x[2]]["path"] and C[x[1]]["name"] == C[x[2]]["name"]]
inv = set(); [inv.update((i, j)) for _, i, j in xf]
print(f"\nNEAR (Jaccard >= {TH} on {K}-token shingles, not exact): pairs={len(pairs)} cross-file={len(xf)} cross-context={len(xc)} sibling-clauses-of-same-function={len(same_fn)} | clauses with a cross-file near twin={len(inv)} minhash_s={time.monotonic()-t1:.1f}")
print("-- largest cross-context near pairs:")
for jac, i, j in sorted(xc, key=lambda x: -min(len(C[x[1]]["l2"]), len(C[x[2]]["l2"])))[:10]:
    print(f"  {jac:.2f}\n      {loc(i)}\n      {loc(j)}")
any_twin = set(inv); [any_twin.update(g) for g in f2]
print(f"\nclauses with any cross-file twin (exact-loose or near): {len(any_twin)} of {len(C)} = {100*len(any_twin)/len(C):.1f}%")
by_ctx = collections.Counter(C[i]["ctx"] for i in any_twin)
print("top contexts:", by_ctx.most_common(12))
