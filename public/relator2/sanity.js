import { getWd } from './app-state.js';
import {
    isAncestorOf,
    isDeprecatedClaim,
    isFemale,
    isMale,
    isSiblingOf,
    isSpouseOf,
    labelForProperty,
    liveRelationItems,
    named_properties,
} from './relations.js';

// Sanity checks for logically impossible relationships (issue #75). Each check
// is conservative: with uncertain dates it warns only when the impossibility
// holds for *every* interpretation allowed by the stated precision, so a
// year-only date never produces a false positive against another year-only one.

const np = named_properties;

// Relations that, between two people, would make them a couple.
const PARTNER_PROPS = [np.spouse, np.unmarried_partner];
// Blood relations: adding one between an existing couple is a red flag.
const BLOOD_PROPS = [np.father, np.mother, np.child, np.sibling];
// Biological parent/child relations. Godparent and stepparent are deliberately
// excluded from the sibling conflict below: an older sibling is commonly a
// younger one's godparent, so those links may legitimately coexist.
const BIO_ANCESTRY_PROPS = [np.father, np.mother, np.child];

// A child can be born this long (in years) after the father's death and still
// be biologically plausible; a mother must be alive at the birth (grace 0).
const FATHER_POSTHUMOUS_GRACE_YEARS = 1;

// Extra leeway added to every date threshold when a contradiction is used to
// *suppress* a suggestion rather than merely warn about it. Hiding a suggestion
// takes the decision away from the user, so the contradiction must hold with a
// year to spare — enough to absorb sloppy sources and calendar quirks.
export const SUPPRESSION_GRACE_YEARS = 1;

// Warning codes that state a physical impossibility, as opposed to a red flag
// worth a human's attention. Only these justify dropping a suggestion outright.
// The structural warnings deliberately stay out: a spouse who is also a sibling
// (couple_siblings) or another blood relative (blood_couple) is worth flagging,
// but dynastic marriages did happen, so those suggestions remain visible.
const IMPOSSIBLE_CODES = new Set([
    'self',
    'couple_no_overlap',
    'child_after_parent_death',
    'parent_after_child',
]);

// Relations exempt from the "same person related more than once" check: a
// person is routinely the teacher (or student) of many people, and being both
// teacher and student of the same person is conceivable, so those links never
// signal a data error. Every other relation property counts — a single target
// appearing twice (e.g. spouse twice, or spouse and sibling) almost always does.
const DUPLICATE_EXEMPT_PROPS = [np.student_of, np.student];

// --- pure date-interval math -------------------------------------------------

// Convert a Wikidata time datavalue (`{ time, precision }`) to a `{ lo, hi }`
// interval on a continuous "fractional year" scale that brackets every instant
// the value could denote. `hi` is exclusive (a year-precision 1850 spans
// [1850, 1851)). Precisions coarser than a year (decade, century, …) are too
// vague to contradict anything, so they yield null and are skipped.
export function timeValueToInterval(value) {
    const time = value?.time;
    const precision = value?.precision;
    if (typeof time !== 'string' || typeof precision !== 'number') return null;
    const m = time.match(/^([+-]?)(\d+)-(\d{2})-(\d{2})/);
    if (!m) return null;
    const year = (m[1] === '-' ? -1 : 1) * Number(m[2]);
    const month = Number(m[3]);
    const day = Number(m[4]);
    if (precision < 9) return null;
    if (precision === 9) return { lo: year, hi: year + 1 };
    if (precision === 10) {
        const lo = year + (month - 1) / 12;
        return { lo, hi: lo + 1 / 12 };
    }
    // Day precision (11) or finer; treat day-of-month within a /31 slot.
    const lo = year + ((month - 1) + (day - 1) / 31) / 12;
    return { lo, hi: lo + 1 / 372 };
}

// Two lifespans provably never overlap: one person's latest possible death is
// at least `gapYears` before the other's earliest possible birth.
export function lifespansDisjoint(birthA, deathA, birthB, deathB, gapYears = 0) {
    if (deathA && birthB && birthB.lo - deathA.hi >= gapYears) return true;
    if (deathB && birthA && birthA.lo - deathB.hi >= gapYears) return true;
    return false;
}

// The child's earliest possible birth is at least `graceYears` after the
// parent's latest possible death.
export function childBornAfterParentDeath(childBirth, parentDeath, graceYears) {
    if (!childBirth || !parentDeath) return false;
    return childBirth.lo - parentDeath.hi >= graceYears;
}

// `a` provably begins at least `gapYears` after `b` ends (a is born after b).
export function bornStrictlyAfter(a, b, gapYears = 0) {
    return !!a && !!b && a.lo - b.hi >= gapYears;
}

// --- relation-level checks ---------------------------------------------------

function intervalFor(wd, q, prop, dateOverrides) {
    const override = dateOverrides[q];
    if (override) {
        if (prop === np.birth_date) return override.birth ?? null;
        if (prop === np.death_date) return override.death ?? null;
    }
    const item = wd?.getItem?.(q);
    if (!item) return null;
    for (const claim of item.getClaimsForProperty(prop)) {
        if (isDeprecatedClaim(claim)) continue;
        const iv = timeValueToInterval(claim?.mainsnak?.datavalue?.value);
        if (iv) return iv;
    }
    return null;
}

// True when q1 and q2 already stand in a biological parent/child relationship,
// in either direction, per the live data. Unlike isAncestorOf this ignores
// godparent/stepparent links (which can legitimately coexist with siblinghood).
function bioParentChild(q1, q2, wd) {
    const linked = (a, b) => {
        const item = wd?.getItem?.(a);
        return !!item && BIO_ANCESTRY_PROPS.some((prop) => liveRelationItems(item, prop).includes(b));
    };
    return linked(q1, q2) || linked(q2, q1);
}

// Map a proposed relation to (parentQ, childQ, mode) for biological-parent
// checks, or null when the relation isn't a biological parentage. P40 (child)
// names no gender, so the grace period is read from the parent's sex (P21),
// defaulting to the lenient father rule when unknown.
function parentChildRoles(prop, from, to) {
    if (prop === np.father) return { parentQ: to, childQ: from, mode: 'father' };
    if (prop === np.mother) return { parentQ: to, childQ: from, mode: 'mother' };
    if (prop === np.child) {
        const mode = isMale(from) ? 'father' : isFemale(from) ? 'mother' : 'father';
        return { parentQ: from, childQ: to, mode };
    }
    return null;
}

// Warnings for a single proposed relation `{ from, prop, to }`. `opts.labels`
// overrides item labels (used for a not-yet-created item); `opts.dateOverrides`
// supplies birth/death intervals for items not (yet) in the store;
// `opts.graceBonusYears` widens every date threshold (see isImpossibleRelation).
export function relationWarnings(rel, wd = getWd(), opts = {}) {
    const out = [];
    if (!rel || !rel.from || !rel.to) return out;
    const { from, prop, to } = rel;
    const dateOverrides = opts.dateOverrides ?? {};
    const graceBonus = opts.graceBonusYears ?? 0;
    const labels = opts.labels ?? {};
    const label = (q) => labels[q] ?? wd?.getItem?.(q)?.getLabel?.() ?? q;
    const a = label(from);
    const b = label(to);
    const propLabel = labelForProperty(prop, wd);
    const add = (code, message) => out.push({ code, message });

    if (from === to) {
        add('self', `${a} cannot be their own ${propLabel}.`);
        return out;
    }

    if (PARTNER_PROPS.includes(prop)) {
        if (isSiblingOf(from, to)) {
            add('couple_siblings', `${a} and ${b} are siblings and cannot also be spouses or partners.`);
        } else if (isAncestorOf(from, to) || isAncestorOf(to, from)) {
            add('couple_parent_child', `${a} and ${b} are in a parent–child relationship and cannot also be spouses or partners.`);
        }
        const birthA = intervalFor(wd, from, np.birth_date, dateOverrides);
        const deathA = intervalFor(wd, from, np.death_date, dateOverrides);
        const birthB = intervalFor(wd, to, np.birth_date, dateOverrides);
        const deathB = intervalFor(wd, to, np.death_date, dateOverrides);
        if (lifespansDisjoint(birthA, deathA, birthB, deathB, graceBonus)) {
            add('couple_no_overlap', `${a} and ${b} were never alive at the same time (one died before the other was born), so they cannot be spouses or partners.`);
        }
    }

    if (BLOOD_PROPS.includes(prop) && isSpouseOf(from, to)) {
        add('blood_couple', `${a} and ${b} are spouses; adding a “${propLabel}” relation would make a married couple blood relatives.`);
    }

    // Siblinghood and (biological) parent/child are mutually exclusive: the same
    // two people cannot be both. The conflicting link existing in either
    // direction makes the new relation impossible (issue #75: a person's father
    // was allowed to be added as their brother, and vice versa).
    if (prop === np.sibling && bioParentChild(from, to, wd)) {
        add('sibling_parent_child', `${a} and ${b} are in a parent–child relationship and cannot also be siblings.`);
    }
    if (BIO_ANCESTRY_PROPS.includes(prop) && isSiblingOf(from, to)) {
        add('parent_child_sibling', `${a} and ${b} are siblings and cannot also be in a parent–child relationship.`);
    }

    const roles = parentChildRoles(prop, from, to);
    if (roles) {
        const { parentQ, childQ, mode } = roles;
        const parentLabel = label(parentQ);
        const childLabel = label(childQ);
        const parentBirth = intervalFor(wd, parentQ, np.birth_date, dateOverrides);
        const parentDeath = intervalFor(wd, parentQ, np.death_date, dateOverrides);
        const childBirth = intervalFor(wd, childQ, np.birth_date, dateOverrides);
        const grace = (mode === 'father' ? FATHER_POSTHUMOUS_GRACE_YEARS : 0) + graceBonus;
        if (childBornAfterParentDeath(childBirth, parentDeath, grace)) {
            add('child_after_parent_death', mode === 'father'
                ? `${childLabel} was born more than a year after their father ${parentLabel} died.`
                : `${childLabel} was born after their mother ${parentLabel} died.`);
        }
        if (bornStrictlyAfter(parentBirth, childBirth, graceBonus)) {
            add('parent_after_child', `${parentLabel} was born after their child ${childLabel}.`);
        }
    }

    return out;
}

// Whether a proposed relation is not merely questionable but impossible, so it
// should never be offered as a suggestion at all: a woman who died years before
// a child was born is not a "maybe" mother. Every threshold gets
// SUPPRESSION_GRACE_YEARS of extra slack, because a hidden suggestion is one the
// user can no longer review — when in doubt, keep showing it.
//
// The underlying date math is interval-based, so mixed precisions are safe: a
// value is turned into the whole span it could denote (year, month or day), and
// precisions coarser than a year (decade, century, …) produce no interval at all
// and therefore never suppress anything.
export function isImpossibleRelation(rel, wd = getWd(), opts = {}) {
    const warnings = relationWarnings(rel, wd, { ...opts, graceBonusYears: SUPPRESSION_GRACE_YEARS });
    return warnings.some((w) => IMPOSSIBLE_CODES.has(w.code));
}

// Within one item's relation set, the target Q-ids that appear in more than one
// non-teacher/student relation. somevalue/novalue targets carry no identity and
// are ignored. `relations` must all share the same `from` item.
export function duplicateRelationTargets(relations) {
    const counts = new Map();
    for (const rel of relations ?? []) {
        if (!rel || DUPLICATE_EXEMPT_PROPS.includes(rel.prop)) continue;
        if (!/^Q\d+$/.test(rel.to ?? '')) continue;
        counts.set(rel.to, (counts.get(rel.to) ?? 0) + 1);
    }
    return new Set([...counts].filter(([, n]) => n > 1).map(([q]) => q));
}

// Item-level warnings (one per over-used target) complementing relationWarnings,
// which only sees a single relation in isolation. `relations` must be all the
// relations sharing one `from` item. Each warning carries `target` so callers
// can attach it to the relevant rows.
export function itemRelationWarnings(relations, wd = getWd(), opts = {}) {
    const labels = opts.labels ?? {};
    const label = (q) => labels[q] ?? wd?.getItem?.(q)?.getLabel?.() ?? q;
    const out = [];
    for (const to of duplicateRelationTargets(relations)) {
        const fromQ = (relations ?? []).find((r) => r?.to === to)?.from;
        out.push({
            code: 'duplicate_person',
            target: to,
            message: `${label(to)} is connected to ${label(fromQ)} through more than one relationship.`,
        });
    }
    return out;
}
