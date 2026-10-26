// Pure (DOM-free, Vue-free) auto-layout for the "Diagram" people layout.
//
// Produces a classic ancestor-tree style positioning: generation dictates the
// Y axis (older = higher), couples and siblings share a generation, and a
// "union" node is inserted between two partners that have at least one child
// together so their shared children hang off the union instead of off the two
// parents individually.
//
// Input model (all QIDs; only relations among `people` are honoured):
//   {
//     people:      [q, ...],
//     parentChild: [{ parent, child, kind, state }, ...],
//     couples:     [{ a, b, kind, state }, ...],   // kind: 'spouse' | 'partner'
//     siblings:    [{ a, b }, ...],
//   }
//
// Output:
//   {
//     pos:     Map<q, {x, y}>,           // top-left of each person box
//     unions:  [{ id, x, y, a, b, kind, state, children:[q,...] }],
//     covered: Set<"parent|child">,      // parent→child links drawn via a union
//     width, height,                     // canvas size in px
//     metrics,                           // the metrics used (box sizes etc.)
//   }
//
// Keeping this separate from the Vue component makes the geometry unit-testable
// and keeps the component focused on rendering and interaction.

export const GRAPH_METRICS = {
    boxW: 156,
    boxH: 58,
    hGap: 26,        // gap between sibling boxes in a generation
    classGap: 30,    // extra gap between unrelated clusters in a generation
    vGap: 84,        // gap between generations (the union node lives in here)
    unionSize: 24,
    margin: 28,
};

const qNum = (q) => Number(String(q).slice(1)) || 0;
const byQid = (a, b) => qNum(a) - qNum(b);

// Minimal union-find for grouping same-generation people.
function makeUnionFind(ids) {
    const parent = new Map(ids.map((id) => [id, id]));
    const find = (x) => {
        let r = x;
        while (parent.get(r) !== r) r = parent.get(r);
        while (parent.get(x) !== r) { const n = parent.get(x); parent.set(x, r); x = n; }
        return r;
    };
    const union = (a, b) => {
        const ra = find(a);
        const rb = find(b);
        if (ra !== rb) parent.set(ra, rb);
    };
    return { find, union };
}

export function computeGraphLayout(model, metrics = GRAPH_METRICS, orientation = 'vertical') {
    const M = metrics;
    const people = [...new Set(model.people ?? [])];
    const peopleSet = new Set(people);
    const inPool = (q) => peopleSet.has(q);

    const empty = { pos: new Map(), unions: [], covered: new Set(), width: 0, height: 0, metrics: M };
    if (people.length === 0) return empty;

    // Parent/child adjacency among pooled people.
    const childrenOf = new Map(people.map((q) => [q, new Set()]));
    const parentsOf = new Map(people.map((q) => [q, new Set()]));
    for (const pc of model.parentChild ?? []) {
        if (!inPool(pc.parent) || !inPool(pc.child) || pc.parent === pc.child) continue;
        childrenOf.get(pc.parent).add(pc.child);
        parentsOf.get(pc.child).add(pc.parent);
    }

    // Same-generation equivalence classes: couples + siblings.
    const uf = makeUnionFind(people);
    for (const c of model.couples ?? []) if (inPool(c.a) && inPool(c.b)) uf.union(c.a, c.b);
    for (const s of model.siblings ?? []) if (inPool(s.a) && inPool(s.b)) uf.union(s.a, s.b);
    const classOf = (q) => uf.find(q);

    const classMembers = new Map();
    for (const q of people) {
        const c = classOf(q);
        if (!classMembers.has(c)) classMembers.set(c, []);
        classMembers.get(c).push(q);
    }
    const classList = [...classMembers.keys()];

    // Class graph (parent class → child class).
    const classChildren = new Map(classList.map((c) => [c, new Set()]));
    const classParents = new Map(classList.map((c) => [c, new Set()]));
    for (const [child, parents] of parentsOf) {
        const cc = classOf(child);
        for (const p of parents) {
            const pc = classOf(p);
            if (pc === cc) continue;
            classChildren.get(pc).add(cc);
            classParents.get(cc).add(pc);
        }
    }

    // Rank = longest path from the roots (older generations get lower ranks, so
    // they end up higher on screen). Iteration is capped at the class count so
    // a cyclic/contradictory data set can never spin forever.
    const rank = new Map(classList.map((c) => [c, 0]));
    for (let iter = 0; iter < classList.length; iter++) {
        let changed = false;
        for (const c of classList) {
            const r = rank.get(c);
            for (const cc of classChildren.get(c)) {
                if (rank.get(cc) < r + 1) { rank.set(cc, r + 1); changed = true; }
            }
        }
        if (!changed) break;
    }
    const maxRank = Math.max(0, ...rank.values());

    // Order members within a class so couple/sibling partners stay adjacent
    // (DFS over the intra-class couple+sibling adjacency).
    const intraAdj = new Map(people.map((q) => [q, new Set()]));
    const addIntra = (a, b) => {
        if (inPool(a) && inPool(b) && classOf(a) === classOf(b)) {
            intraAdj.get(a).add(b);
            intraAdj.get(b).add(a);
        }
    };
    for (const c of model.couples ?? []) addIntra(c.a, c.b);
    for (const s of model.siblings ?? []) addIntra(s.a, s.b);
    const orderMembers = (members) => {
        const set = new Set(members);
        const seen = new Set();
        const out = [];
        for (const start of [...members].sort(byQid)) {
            if (seen.has(start)) continue;
            const stack = [start];
            while (stack.length) {
                const x = stack.pop();
                if (seen.has(x)) continue;
                seen.add(x);
                out.push(x);
                const nb = [...intraAdj.get(x)].filter((n) => set.has(n) && !seen.has(n)).sort(byQid);
                for (let i = nb.length - 1; i >= 0; i--) stack.push(nb[i]);
            }
        }
        return out;
    };
    const membersOrdered = new Map(classList.map((c) => [c, orderMembers(classMembers.get(c))]));

    // Group classes into generation levels, seeded left→right by smallest QID.
    const minQ = (c) => Math.min(...classMembers.get(c).map(qNum));
    const levels = Array.from({ length: maxRank + 1 }, () => []);
    for (const c of classList) levels[rank.get(c)].push(c);
    for (const lvl of levels) lvl.sort((a, b) => minQ(a) - minQ(b));

    // Lay out in abstract (breadth, depth) space — depth = generation, breadth
    // = ordering within a generation — then map to (x, y) per orientation at the
    // very end. This keeps the algorithm identical for both orientations.
    const horizontal = orientation === 'horizontal';
    const breadthExtent = horizontal ? M.boxH : M.boxW; // box size across a generation
    const depthExtent = horizontal ? M.boxW : M.boxH;   // box size along the flow
    const breadthStep = breadthExtent + M.hGap;
    const depthStep = depthExtent + M.vGap;

    const bpos = new Map();
    const assign = () => {
        for (let r = 0; r <= maxRank; r++) {
            let cursor = M.margin;
            const d = M.margin + r * depthStep;
            for (const c of levels[r]) {
                for (const q of membersOrdered.get(c)) {
                    bpos.set(q, { b: cursor, d });
                    cursor += breadthStep;
                }
                cursor += M.classGap;
            }
        }
    };
    const centerB = (q) => bpos.get(q).b + breadthExtent / 2;

    // Barycenter ordering: a handful of down/up sweeps pulls each cluster
    // towards the average position of its parents (down) and children (up),
    // which keeps related items close and reduces edge crossings.
    const baryKey = (c, relMap) => {
        let sum = 0;
        let n = 0;
        for (const rc of relMap.get(c)) {
            for (const q of classMembers.get(rc)) { sum += centerB(q); n++; }
        }
        if (n > 0) return sum / n;
        const mem = membersOrdered.get(c);
        return mem.reduce((s, q) => s + centerB(q), 0) / mem.length;
    };
    const sortByBary = (arr, relMap) => arr
        .map((c, i) => ({ c, i, k: baryKey(c, relMap) }))
        .sort((a, b) => a.k - b.k || a.i - b.i)
        .map((o) => o.c);

    assign();
    for (let sweep = 0; sweep < 4; sweep++) {
        for (let r = 1; r <= maxRank; r++) { levels[r] = sortByBary(levels[r], classParents); assign(); }
        for (let r = maxRank - 1; r >= 0; r--) { levels[r] = sortByBary(levels[r], classChildren); assign(); }
    }

    // Centre every generation across the breadth axis for a tidy symmetric look.
    let maxB = 0;
    for (const p of bpos.values()) maxB = Math.max(maxB, p.b + breadthExtent);
    const contentB = maxB - M.margin;
    for (let r = 0; r <= maxRank; r++) {
        const ps = levels[r].flatMap((c) => membersOrdered.get(c)).map((q) => bpos.get(q));
        if (ps.length === 0) continue;
        const lo = Math.min(...ps.map((p) => p.b));
        const hi = Math.max(...ps.map((p) => p.b + breadthExtent));
        const shift = M.margin + (contentB - (hi - lo)) / 2 - lo;
        for (const p of ps) p.b += shift;
    }

    // Union nodes: one per couple that shares at least one child, placed one rung
    // further along the depth axis and centred between the partners across the
    // breadth axis. The shared children connect to the union instead of to each
    // parent individually.
    const seenCouple = new Set();
    const rawUnions = [];
    const covered = new Set();
    let uid = 0;
    for (const cp of model.couples ?? []) {
        if (!inPool(cp.a) || !inPool(cp.b)) continue;
        const key = [cp.a, cp.b].sort(byQid).join('|');
        if (seenCouple.has(key)) continue;
        seenCouple.add(key);
        const shared = [...childrenOf.get(cp.a)].filter((c) => childrenOf.get(cp.b).has(c));
        if (shared.length === 0) continue;
        const ub = (centerB(cp.a) + centerB(cp.b)) / 2 - M.unionSize / 2;
        const ud = Math.max(bpos.get(cp.a).d, bpos.get(cp.b).d) + depthExtent + (M.vGap / 2 - M.unionSize / 2);
        rawUnions.push({ id: `u${uid++}`, b: ub, d: ud, a: cp.a, partner: cp.b, kind: cp.kind, state: cp.state, children: shared });
        for (const c of shared) { covered.add(`${cp.a}|${c}`); covered.add(`${cp.b}|${c}`); }
    }

    // Horizontal reads right-to-left: ancestors on the right, descendants on the
    // left. Mirror the depth axis (each element about the full span, preserving
    // its own size) so the oldest generation ends up at the far right. Vertical
    // keeps older = higher.
    if (horizontal) {
        let dMax = 0;
        for (const p of bpos.values()) dMax = Math.max(dMax, p.d + depthExtent);
        for (const u of rawUnions) dMax = Math.max(dMax, u.d + M.unionSize);
        for (const p of bpos.values()) p.d = M.margin + (dMax - (p.d + depthExtent));
        for (const u of rawUnions) u.d = M.margin + (dMax - (u.d + M.unionSize));
    }

    // Map abstract (breadth, depth) → screen (x, y).
    const toXY = (b, d) => (horizontal ? { x: d, y: b } : { x: b, y: d });
    const pos = new Map();
    for (const [q, p] of bpos) pos.set(q, toXY(p.b, p.d));
    const unions = rawUnions.map((u) => {
        const { x, y } = toXY(u.b, u.d);
        return { id: u.id, x, y, a: u.a, b: u.partner, kind: u.kind, state: u.state, children: u.children };
    });

    let width = 0;
    let height = 0;
    for (const p of pos.values()) { width = Math.max(width, p.x + M.boxW); height = Math.max(height, p.y + M.boxH); }
    for (const u of unions) { width = Math.max(width, u.x + M.unionSize); height = Math.max(height, u.y + M.unionSize); }
    width += M.margin;
    height += M.margin;

    return { pos, unions, covered, width, height, metrics: M };
}
