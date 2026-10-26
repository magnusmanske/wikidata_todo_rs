import { ttMixin } from '../../resources/vue_es6/state.js';
import { appState, getWd } from '../app-state.js';
import {
    isMale,
    isFemale,
    mirrorSelectionEvent,
    named_properties,
} from '../relations.js';
import { isRejected } from '../rejection_store.js';
import { computeGraphLayout, GRAPH_METRICS } from '../graph-layout.js';
import './person.js';

const P = named_properties;

// Relation property → edge kind. The kind drives both the line style and the
// tooltip; states (live / selected / maybe) are layered on top for colour.
const PARENT_KIND = { [P.father]: 'father', [P.mother]: 'mother', [P.stepparent]: 'step', [P.godparent]: 'god' };
const COUPLE_KIND = { [P.spouse]: 'spouse', [P.unmarried_partner]: 'partner' };
const OTHER_KIND = { [P.relative]: 'relative', [P.student_of]: 'teacher', [P.student]: 'student', [P.significant_person]: 'significant' };

const KIND_LABEL = {
    father: 'father', mother: 'mother', child: 'child', step: 'stepparent', god: 'godparent',
    sibling: 'sibling', spouse: 'spouse', partner: 'unmarried partner',
    relative: 'relative', teacher: 'teacher', student: 'student', significant: 'significant person',
};

// Among the parent-ish kinds for one parent→child pair, keep the most specific.
const PC_KIND_RANK = { father: 4, mother: 4, step: 3, god: 3, child: 1 };

const STATE_RANK = { live: 3, selected: 2, maybe: 1 };
const strongerState = (a, b) => (STATE_RANK[a] >= STATE_RANK[b] ? a : b);

const qNum = (q) => Number(String(q).slice(1)) || 0;
const byQid = (a, b) => qNum(a) - qNum(b);
const coupleKey = (a, b) => [a, b].sort(byQid).join('|');

const ORIENTATION_KEY = 'relator2_graph_orientation';
function loadOrientation() {
    try { return localStorage.getItem(ORIENTATION_KEY) === 'vertical' ? 'vertical' : 'horizontal'; }
    catch { return 'horizontal'; }
}
function storeOrientation(v) {
    try { localStorage.setItem(ORIENTATION_KEY, v); } catch { /* storage unavailable */ }
}

const definition = {
    name: 'graph-view',
    mixins: [ttMixin],
    props: ['suggestions', 'is_busy', 'editing'],
    data() {
        return { expanded_q: null, drag_over_q: '', orientation: loadOrientation() };
    },
    computed: {
        main_q() { return appState.main_q; },
        hovered_q() { return appState.hovered_q; },
        metrics() { return GRAPH_METRICS; },

        // Classify every live relation and (non-rejected) suggestion into the
        // structural buckets the diagram understands, de-duplicating mirrored
        // statements and keeping the strongest state per edge.
        model() {
            const peopleSet = new Set(appState.items);
            const inPool = (q) => peopleSet.has(q) && /^Q\d+$/.test(q);

            const rels = [];
            for (const q of appState.items) {
                for (const r of (appState.item_relations[q] ?? [])) {
                    rels.push({ from: r.from, prop: r.prop, to: r.to, state: 'live', sugg: null });
                }
            }
            for (const s of (this.suggestions ?? [])) {
                if (s.suggestion && isRejected(s)) continue;
                rels.push({ from: s.from, prop: s.prop, to: s.to, state: s.selected ? 'selected' : 'maybe', sugg: s });
            }

            const pcMap = new Map();
            const coupleMap = new Map();
            const sibMap = new Map();
            const otherMap = new Map();
            const bump = (map, key, make) => {
                let e = map.get(key);
                if (!e) { e = make(); map.set(key, e); }
                return e;
            };

            for (const r of rels) {
                if (!inPool(r.from) || !inPool(r.to) || r.from === r.to) continue;

                if (PARENT_KIND[r.prop]) {
                    const parent = r.to;
                    const child = r.from;
                    const kind = PARENT_KIND[r.prop];
                    const e = bump(pcMap, `${parent}|${child}`, () => ({ parent, child, kind, state: r.state, suggs: [] }));
                    if (PC_KIND_RANK[kind] > PC_KIND_RANK[e.kind]) e.kind = kind;
                    e.state = strongerState(e.state, r.state);
                    if (r.sugg) e.suggs.push(r.sugg);
                } else if (r.prop === P.child) {
                    const parent = r.from;
                    const child = r.to;
                    const e = bump(pcMap, `${parent}|${child}`, () => ({ parent, child, kind: 'child', state: r.state, suggs: [] }));
                    e.state = strongerState(e.state, r.state);
                    if (r.sugg) e.suggs.push(r.sugg);
                } else if (COUPLE_KIND[r.prop]) {
                    const [a, b] = [r.from, r.to].sort(byQid);
                    const e = bump(coupleMap, coupleKey(a, b), () => ({ a, b, kind: COUPLE_KIND[r.prop], state: r.state, suggs: [] }));
                    if (COUPLE_KIND[r.prop] === 'spouse') e.kind = 'spouse';
                    e.state = strongerState(e.state, r.state);
                    if (r.sugg) e.suggs.push(r.sugg);
                } else if (r.prop === P.sibling) {
                    const [a, b] = [r.from, r.to].sort(byQid);
                    const e = bump(sibMap, coupleKey(a, b), () => ({ a, b, state: r.state, suggs: [] }));
                    e.state = strongerState(e.state, r.state);
                    if (r.sugg) e.suggs.push(r.sugg);
                } else if (OTHER_KIND[r.prop]) {
                    const kind = OTHER_KIND[r.prop];
                    const [a, b] = [r.from, r.to].sort(byQid);
                    const e = bump(otherMap, `${coupleKey(a, b)}|${kind}`, () => ({ a, b, kind, state: r.state, suggs: [] }));
                    e.state = strongerState(e.state, r.state);
                    if (r.sugg) e.suggs.push(r.sugg);
                }
            }

            const parentsOf = new Map();
            for (const e of pcMap.values()) {
                if (!parentsOf.has(e.child)) parentsOf.set(e.child, new Set());
                parentsOf.get(e.child).add(e.parent);
            }

            return {
                people: [...appState.items],
                parentChild: [...pcMap.values()],
                couples: [...coupleMap.values()],
                siblings: [...sibMap.values()],
                others: [...otherMap.values()],
                pcMap,
                parentsOf,
            };
        },

        layout() {
            const m = this.model;
            return computeGraphLayout({
                people: m.people,
                parentChild: m.parentChild,
                couples: m.couples,
                siblings: m.siblings,
            }, this.metrics, this.orientation);
        },

        nodes() {
            return appState.items.filter((q) => this.layout.pos.has(q));
        },

        // SVG path descriptors for every connecting line, with kind/state for
        // styling and the underlying suggestion objects for click-to-toggle.
        edges() {
            const M = this.metrics;
            const { pos, unions, covered } = this.layout;
            const model = this.model;
            const has = (q) => pos.has(q);
            const horizontal = this.orientation === 'horizontal';
            // Generation links leave a box on the child-ward side and arrive on
            // the parent-ward side — bottom/top when vertical; when horizontal,
            // ancestors sit on the right, so child-ward is the left edge and
            // parent-ward is the right edge.
            const childFace = (q) => (horizontal
                ? { x: pos.get(q).x, y: pos.get(q).y + M.boxH / 2 }
                : { x: pos.get(q).x + M.boxW / 2, y: pos.get(q).y + M.boxH });
            const parentFace = (q) => (horizontal
                ? { x: pos.get(q).x + M.boxW, y: pos.get(q).y + M.boxH / 2 }
                : { x: pos.get(q).x + M.boxW / 2, y: pos.get(q).y });
            const ctr = (q) => ({ x: pos.get(q).x + M.boxW / 2, y: pos.get(q).y + M.boxH / 2 });
            const uCtr = (u) => ({ x: u.x + M.unionSize / 2, y: u.y + M.unionSize / 2 });
            // Spouses attach to the two breadth-axis *sides* of the union node
            // (opposite each other) — left/right when vertical, top/bottom when
            // horizontal — while children stay on the flow-axis front (uCtr).
            const unionSide = (u, p) => {
                if (horizontal) {
                    const lower = pos.get(p).y + M.boxH / 2 <= u.y + M.unionSize / 2;
                    return { x: u.x + M.unionSize / 2, y: lower ? u.y : u.y + M.unionSize };
                }
                const lower = pos.get(p).x + M.boxW / 2 <= u.x + M.unionSize / 2;
                return { x: lower ? u.x : u.x + M.unionSize, y: u.y + M.unionSize / 2 };
            };
            // Smooth S-curve bending along the flow axis. Midpoint control points
            // keep it correct whichever way the link runs (left- or rightward).
            const flow = (p1, p2) => {
                if (horizontal) {
                    const mx = (p1.x + p2.x) / 2;
                    return `M${p1.x},${p1.y} C${mx},${p1.y} ${mx},${p2.y} ${p2.x},${p2.y}`;
                }
                const my = (p1.y + p2.y) / 2;
                return `M${p1.x},${p1.y} C${p1.x},${my} ${p2.x},${my} ${p2.x},${p2.y}`;
            };
            const seg = (p1, p2) => `M${p1.x},${p1.y} L${p2.x},${p2.y}`;
            // Spouse → union: leave the spouse along the flow axis, but arrive
            // perpendicular *into* the union side (control point pulled straight
            // out from that side) rather than grazing it tangentially.
            const spouseLink = (p1, end, u) => {
                const k = M.unionSize + 8;
                if (horizontal) {
                    const top = end.y < u.y + M.unionSize / 2;
                    return `M${p1.x},${p1.y} C${(p1.x + end.x) / 2},${p1.y} ${end.x},${end.y + (top ? -k : k)} ${end.x},${end.y}`;
                }
                const left = end.x < u.x + M.unionSize / 2;
                return `M${p1.x},${p1.y} C${p1.x},${(p1.y + end.y) / 2} ${end.x + (left ? -k : k)},${end.y} ${end.x},${end.y}`;
            };
            const out = [];
            const unionKeys = new Set();

            for (const u of unions) {
                unionKeys.add(coupleKey(u.a, u.b));
                const uc = uCtr(u);
                for (const p of [u.a, u.b]) {
                    if (!has(p)) continue;
                    out.push({
                        key: `u-${u.id}-${p}`, kind: u.kind, state: u.state, d: spouseLink(childFace(p), unionSide(u, p), u),
                        qs: [u.a, u.b], suggs: this.coupleSuggs(u.a, u.b), title: KIND_LABEL[u.kind],
                    });
                }
                for (const c of u.children) {
                    if (!has(c)) continue;
                    const st = this.childUnionState(u, c);
                    out.push({
                        key: `u-${u.id}-c-${c}`, kind: 'child', state: st.state, d: flow(uc, parentFace(c)),
                        qs: [c], suggs: st.suggs, title: KIND_LABEL.child,
                    });
                }
            }

            for (const e of model.parentChild) {
                if (!has(e.parent) || !has(e.child)) continue;
                if (covered.has(`${e.parent}|${e.child}`)) continue;
                out.push({
                    key: `pc-${e.parent}-${e.child}`, kind: e.kind, state: e.state, d: flow(childFace(e.parent), parentFace(e.child)),
                    qs: [e.parent, e.child], suggs: e.suggs, title: KIND_LABEL[e.kind],
                });
            }

            for (const e of model.couples) {
                if (!has(e.a) || !has(e.b)) continue;
                if (unionKeys.has(coupleKey(e.a, e.b))) continue;
                out.push({
                    key: `cp-${e.a}-${e.b}`, kind: e.kind, state: e.state, d: seg(ctr(e.a), ctr(e.b)),
                    qs: [e.a, e.b], suggs: e.suggs, title: KIND_LABEL[e.kind],
                });
            }

            for (const e of model.siblings) {
                if (!has(e.a) || !has(e.b)) continue;
                if (this.shareParent(e.a, e.b)) continue; // implied by the tree already
                out.push({
                    key: `sb-${e.a}-${e.b}`, kind: 'sibling', state: e.state, d: seg(ctr(e.a), ctr(e.b)),
                    qs: [e.a, e.b], suggs: e.suggs, title: KIND_LABEL.sibling,
                });
            }

            for (const e of model.others) {
                if (!has(e.a) || !has(e.b)) continue;
                out.push({
                    key: `ot-${e.a}-${e.b}-${e.kind}`, kind: e.kind, state: e.state, d: seg(ctr(e.a), ctr(e.b)),
                    qs: [e.a, e.b], suggs: e.suggs, title: KIND_LABEL[e.kind],
                });
            }

            return out;
        },
        clickableEdges() {
            return this.edges.filter((e) => e.suggs.length > 0);
        },
        canvasStyle() {
            return { width: `${this.layout.width}px`, height: `${this.layout.height}px` };
        },
        // Per-node count of actionable suggestion rows (the checkboxes shown in
        // the expanded card): non-transient, non-rejected suggestions on q.
        suggestionCounts() {
            const counts = {};
            for (const s of (this.suggestions ?? [])) {
                if (!s.suggestion || s.transient || isRejected(s)) continue;
                counts[s.from] = (counts[s.from] ?? 0) + 1;
            }
            return counts;
        },
    },
    methods: {
        coupleSuggs(a, b) {
            const key = coupleKey(a, b);
            const e = this.model.couples.find((c) => coupleKey(c.a, c.b) === key);
            return e ? e.suggs : [];
        },
        // Combine the states of the two parent→child links feeding a union so
        // the union→child line is coloured by the firmer of the pair.
        childUnionState(u, c) {
            const states = [];
            const suggs = [];
            for (const p of [u.a, u.b]) {
                const e = this.model.pcMap.get(`${p}|${c}`);
                if (e) { states.push(e.state); suggs.push(...e.suggs); }
            }
            const state = states.reduce((acc, s) => strongerState(acc, s), 'maybe');
            return { state, suggs };
        },
        shareParent(a, b) {
            const pa = this.model.parentsOf.get(a);
            const pb = this.model.parentsOf.get(b);
            if (!pa || !pb) return false;
            for (const p of pa) if (pb.has(p)) return true;
            return false;
        },

        nodeStyle(q) {
            const p = this.layout.pos.get(q);
            return { left: `${p.x}px`, top: `${p.y}px`, width: `${this.metrics.boxW}px`, height: `${this.metrics.boxH}px` };
        },
        expandedStyle(q) {
            const p = this.layout.pos.get(q);
            return { left: `${p.x}px`, top: `${p.y}px`, minWidth: `${this.metrics.boxW}px` };
        },
        unionStyle(u) {
            return { left: `${u.x}px`, top: `${u.y}px`, width: `${this.metrics.unionSize}px`, height: `${this.metrics.unionSize}px` };
        },
        unionIcon(kind) { return kind === 'partner' ? '⚯' : '⚭'; }, // ⚯ / ⚭
        title_for_kind(kind) { return KIND_LABEL[kind] || kind; },
        unionClass(u) {
            return ['relator-union', `gstate-${u.state}`, {
                'relator-union-hot': u.a === this.hovered_q || u.b === this.hovered_q,
            }];
        },

        label(q) { return getWd().getItem(q)?.getLabel() || q; },
        yearOf(q, prop) {
            const i = getWd().getItem(q);
            if (!i) return '';
            const claims = i.bestRank(i.getClaimsForProperty(prop, true)) ?? [];
            if (claims.length === 0) return '';
            const best = claims.reduce((b, c) => (c.precision > b.precision ? c : b), claims[0]);
            const v = i.getClaimDate(best);
            const m = v ? String(v.time).match(/^([+-]?\d+)/) : null;
            return m ? String(Number(m[1])) : '';
        },
        years(q) {
            const born = this.yearOf(q, P.birth_date);
            const died = this.yearOf(q, P.death_date);
            if (born === '' && died === '') return '';
            return `${born}–${died}`;
        },
        genderGlyph(q) {
            if (isMale(q)) return '♂';
            if (isFemale(q)) return '♀';
            return getWd().getItem(q)?.hasClaims(P.gender) ? '?' : '';
        },
        nodeClass(q) {
            return [
                isMale(q) ? 'relator-node-male' : (isFemale(q) ? 'relator-node-female' : 'relator-node-unknown'),
                {
                    'relator-node-main': q === this.main_q,
                    'relator-node-hot': q === this.hovered_q,
                    'relator-node-drop': q === this.drag_over_q,
                    'relator-node-busy': this.nodeBusy(q),
                },
            ];
        },
        edgeClass(e) {
            return ['gedge', `gtype-${e.kind}`, `gstate-${e.state}`, {
                'gedge-hot': e.qs.includes(this.hovered_q),
                'gedge-clickable': e.suggs.length > 0,
            }];
        },

        nodeBusy(q) { return (this.is_busy[q] ?? 0) > 0; },
        suggestionsForQ(q) { return (this.suggestions ?? []).filter((s) => s.from === q); },

        onNodeClick(q) { this.expanded_q = this.expanded_q === q ? null : q; },
        closeExpanded() { this.expanded_q = null; },
        // A click on the empty canvas (not a box, union, edge or the expanded
        // card — those are child elements) collapses the expanded item.
        onBackgroundClick(e) { if (e.target === e.currentTarget) this.closeExpanded(); },
        toggleOrientation() {
            this.orientation = this.orientation === 'horizontal' ? 'vertical' : 'horizontal';
            storeOrientation(this.orientation);
        },

        onBoxDragOver(event, q) {
            event.preventDefault();
            event.dataTransfer.dropEffect = 'link';
            this.drag_over_q = q;
        },
        onBoxDragLeave(q) { if (this.drag_over_q === q) this.drag_over_q = ''; },
        // A collapsed box has no add-relation UI, so expand it and hand the
        // dropped URL to the person card, which owns all the drop handling
        // (URL→Qid resolution, drop rules, the relation chooser).
        onBoxDrop(event, q) {
            event.preventDefault();
            this.drag_over_q = '';
            const dt = event.dataTransfer;
            const url = (dt?.getData('text/uri-list') || dt?.getData('text/plain') || '').trim();
            if (!url) return;
            this.expanded_q = q;
            this.$nextTick(() => this.$refs.expandedPerson?.handleDroppedUrl(url));
        },
        onNodeEnter(q) { appState.hovered_q = q; },
        onNodeLeave(q) { if (appState.hovered_q === q) appState.hovered_q = ''; },

        // Toggle a clickable suggestion line: flip its selected flag (mirroring
        // the person card's checkbox) and emit the reciprocal selection so both
        // directions stay in sync. The global "Add checked" button then commits.
        onEdgeClick(e) {
            if (e.suggs.length === 0) return;
            const target = !e.suggs.every((s) => s.selected);
            for (const s of e.suggs) {
                s.selected = target;
                const ev = mirrorSelectionEvent(s, target);
                if (ev) this.$emit('select-suggestion', ev);
            }
        },

        forwardAddSuggestions(ev) { this.$emit('add-suggestions', ev); },
        forwardChangeMainQ(ev) { this.$emit('change-main-q', ev); },
        forwardSelectSuggestion(ev) { this.$emit('select-suggestion', ev); },
        forwardSetGender(ev) { this.$emit('set-gender', ev); },
        forwardSetDescription(ev) { this.$emit('set-description', ev); },
    },
    template: `
<div class="relator-graph">
    <div class="relator-graph-legend">
        <span class="relator-graph-hint">Click a box to expand it; click a coloured suggestion line to toggle it, then use "Add checked".</span>
        <span class="relator-legend-item"><svg width="34" height="10"><line class="gedge gtype-father" x1="2" y1="5" x2="32" y2="5"/></svg> parent</span>
        <span class="relator-legend-item"><svg width="34" height="10"><line class="gedge gtype-partner" x1="2" y1="5" x2="32" y2="5"/></svg> partner</span>
        <span class="relator-legend-item"><svg width="34" height="10"><line class="gedge gtype-sibling" x1="2" y1="5" x2="32" y2="5"/></svg> sibling</span>
        <span class="relator-legend-item"><svg width="34" height="10"><line class="gedge gstate-selected" x1="2" y1="5" x2="32" y2="5"/></svg> selected</span>
        <span class="relator-legend-item"><svg width="34" height="10"><line class="gedge gstate-maybe" x1="2" y1="5" x2="32" y2="5"/></svg> maybe</span>
        <button type="button" class="relator-orient-toggle" @click="toggleOrientation"
            :title="orientation === 'horizontal' ? 'Switch to vertical (top-down) layout' : 'Switch to horizontal (left-right) layout'">{{ orientation === 'horizontal' ? '⇅ vertical' : '⇆ horizontal' }}</button>
    </div>
    <div class="relator-graph-canvas" :style="canvasStyle" @click="onBackgroundClick">
        <svg class="relator-graph-edges" :width="layout.width" :height="layout.height">
            <path v-for="e in edges" :key="e.key" :class="edgeClass(e)" :d="e.d" fill="none"><title>{{e.title}}</title></path>
            <path v-for="e in clickableEdges" :key="'hit-' + e.key" class="gedge-hit" :d="e.d" fill="none"
                @click="onEdgeClick(e)"><title>{{e.title}} (click to toggle)</title></path>
        </svg>

        <div v-for="u in layout.unions" :key="u.id" :class="unionClass(u)" :style="unionStyle(u)"
            :title="title_for_kind(u.kind)">{{unionIcon(u.kind)}}</div>

        <template v-for="q in nodes">
            <div v-if="expanded_q !== q" :key="'node-' + q"
                :class="nodeClass(q)" class="relator-node" :style="nodeStyle(q)"
                @click="onNodeClick(q)" @mouseenter="onNodeEnter(q)" @mouseleave="onNodeLeave(q)"
                @dragover="onBoxDragOver($event, q)" @dragleave="onBoxDragLeave(q)" @drop="onBoxDrop($event, q)">
                <div class="relator-node-name" :title="label(q) + ' [' + q + ']'">{{label(q)}}</div>
                <div class="relator-node-meta">
                    <span class="relator-node-years">{{years(q)}}</span>
                    <span class="relator-node-right">
                        <span v-if="suggestionCounts[q]" class="relator-node-badge"
                            :title="suggestionCounts[q] + ' suggestion(s) to review — expand to see them'">☑ {{suggestionCounts[q]}}</span>
                        <span class="relator-node-gender">{{genderGlyph(q)}}</span>
                    </span>
                </div>
            </div>
        </template>

        <div v-if="expanded_q" class="relator-node-expanded" :style="expandedStyle(expanded_q)">
            <button type="button" class="relator-expand-close" title="collapse" @click.stop="closeExpanded">&times;</button>
            <ul class="people-list">
                <person ref="expandedPerson" :q="expanded_q" :hide_relations="true"
                    :is_busy="nodeBusy(expanded_q)" :editing="editing" :suggestions="suggestionsForQ(expanded_q)"
                    @add-suggestions="forwardAddSuggestions" @change-main-q="forwardChangeMainQ"
                    @select-suggestion="forwardSelectSuggestion" @set-gender="forwardSetGender"
                    @set-description="forwardSetDescription"></person>
            </ul>
        </div>
    </div>
</div>`,
};

Vue.component('graph-view', definition);

export default definition;
