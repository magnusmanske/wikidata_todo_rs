import { sortRelations, hasMirrorProperty } from './utils.js';
import { appState, getWd, setItemRelations } from './app-state.js';

export const named_properties = {
    father: 'P22',
    mother: 'P25',
    godparent: 'P1290',
    stepparent: 'P3448',
    child: 'P40',
    spouse: 'P26',
    sibling: 'P3373',
    relative: 'P1038',
    unmarried_partner: 'P451',
    student_of: 'P1066',
    student: 'P802',
    significant_person: 'P3342',
    birth_date: 'P569',
    death_date: 'P570',
    gender: 'P21',
};

export const named_items = {
    male: 'Q6581097',
    female: 'Q6581072',
};

// A deprecated rank is Wikidata's signal that a claim is known to be wrong
// (e.g. a marriage later disproven). Relator2 must not surface such statements
// as live relations: they are dropped from the relations table, the family
// tree, related-item loading, and suggestion generation. Existence guards that
// suppress re-suggesting a relation deliberately keep counting deprecated
// claims, so deprecating a link is never undone by a fresh suggestion.
export function isDeprecatedClaim(claim) {
    return claim?.rank === 'deprecated';
}

// Item targets of non-deprecated claims for a property. Mirrors the library's
// getClaimItemsForProperty(prop, true) — targets are returned even when the
// item is not yet loaded — but drops deprecated statements.
export function liveRelationItems(item, prop) {
    return item.getClaimsForProperty(prop)
        .filter((claim) => !isDeprecatedClaim(claim))
        .map((claim) => claim?.mainsnak?.datavalue?.value)
        .filter((value) => value?.['entity-type'] === 'item')
        .map((value) => value.id);
}

function hasLiveItemLink(item, prop, q) {
    return liveRelationItems(item, prop).includes(q);
}

export const relation_properties = [
    named_properties.father,
    named_properties.mother,
    named_properties.godparent,
    named_properties.stepparent,
    named_properties.child,
    named_properties.spouse,
    named_properties.sibling,
    named_properties.relative,
    named_properties.unmarried_partner,
    named_properties.student_of,
    named_properties.student,
    named_properties.significant_person,
];

// Relations offered with a non-committal "maybe" default: the mirror link is
// plausible but too loose to pre-select for the user. relative (P1038) and
// significant person (P3342) are deliberately broad associations.
export const maybe_default_properties = new Set([
    named_properties.relative,
    named_properties.significant_person,
]);

// Property labels we override for display purposes. P1066's Wikidata label is
// "student of", which reads awkwardly in the dropdowns ("X is the student of
// of Y") — the inverse role label is correct here.
export const property_label_overrides = {
    [named_properties.student_of]: 'teacher',
};

export function labelForProperty(q, wd, lang) {
    if (Object.prototype.hasOwnProperty.call(property_label_overrides, q)) {
        return property_label_overrides[q];
    }
    const i = wd?.getItem?.(q);
    if (!i) return q;
    return lang ? i.getLabel(lang) : i.getLabel();
}

// Maps a relation property to the property/properties of its reciprocal
// statement. A property mapped to an empty array is a recognised relation that
// has no reciprocal property on Wikidata (stepparent P3448 and godparent P1290
// have no "stepchild"/"godchild" property), so only the statement on the other
// item is ever written — but the property is still a key here so the relation
// is displayed (relationClaims iterates these keys) and selectable.
export const mirror_properties = {
    [named_properties.father]: [named_properties.child],
    [named_properties.mother]: [named_properties.child],
    [named_properties.stepparent]: [],
    [named_properties.godparent]: [],
    [named_properties.spouse]: [named_properties.spouse],
    [named_properties.sibling]: [named_properties.sibling],
    [named_properties.relative]: [named_properties.relative],
    [named_properties.unmarried_partner]: [named_properties.unmarried_partner],
    [named_properties.child]: [named_properties.father, named_properties.mother],
    [named_properties.student_of]: [named_properties.student],
    [named_properties.student]: [named_properties.student_of],
    [named_properties.significant_person]: [named_properties.significant_person],
};

export function isMale(q) {
    const i = getWd().getItem(q);
    return !!i && i.hasClaimItemLink(named_properties.gender, named_items.male);
}

export function isFemale(q) {
    const i = getWd().getItem(q);
    return !!i && i.hasClaimItemLink(named_properties.gender, named_items.female);
}

export function isAncestorOf(ancestorQ, descendantQ) {
    const wd = getWd();
    const ancestor   = wd.getItem(ancestorQ);
    const descendant = wd.getItem(descendantQ);
    if (!ancestor || !descendant) return false;
    return hasLiveItemLink(descendant, named_properties.father,     ancestorQ)
        || hasLiveItemLink(descendant, named_properties.mother,     ancestorQ)
        || hasLiveItemLink(descendant, named_properties.godparent,  ancestorQ)
        || hasLiveItemLink(descendant, named_properties.stepparent, ancestorQ)
        || hasLiveItemLink(ancestor,   named_properties.child,      descendantQ);
}

// Biological parent/child ancestry only: father (P22), mother (P25), or an
// explicit child (P40) link. Unlike isAncestorOf this excludes godparent and
// stepparent, which do not make someone a biological descendant. Used when
// suggesting a P40 "child" relation so a step- or god-child is never offered as
// a biological child, and vice versa (issue #79).
export function isBiologicalAncestorOf(ancestorQ, descendantQ) {
    const wd = getWd();
    const ancestor   = wd.getItem(ancestorQ);
    const descendant = wd.getItem(descendantQ);
    if (!ancestor || !descendant) return false;
    return hasLiveItemLink(descendant, named_properties.father, ancestorQ)
        || hasLiveItemLink(descendant, named_properties.mother, ancestorQ)
        || hasLiveItemLink(ancestor,   named_properties.child,  descendantQ);
}

// Item targets already recorded as any kind of parent of `item`: biological
// (father P22 / mother P25) or social (stepparent P3448 / godparent P1290). A
// fresh parent suggestion must never point at one of these — if George is
// explicitly Marian's stepparent, he must not also be offered as her father,
// even though he is married to her mother and is her sibling's father (#79).
export function knownParents(item) {
    return [
        named_properties.father,
        named_properties.mother,
        named_properties.stepparent,
        named_properties.godparent,
    ].flatMap((prop) => liveRelationItems(item, prop));
}

// A person has one father and one mother, so an explicit "unknown value"
// (somevalue) or "no value" statement for P22/P25 already answers the question:
// no concrete item may be suggested for that property (e.g. the mother's
// spouse offered as the father of someone whose father is recorded as unknown).
export function hasUnknownParent(item, prop) {
    if (prop !== named_properties.father && prop !== named_properties.mother) return false;
    return (item?.getClaimsForProperty(prop) ?? [])
        .filter((claim) => !isDeprecatedClaim(claim))
        .some((claim) => ['somevalue', 'novalue'].includes(claim?.mainsnak?.snaktype));
}

export function isSpouseOf(q1, q2) {
    const wd = getWd();
    const i1 = wd.getItem(q1);
    const i2 = wd.getItem(q2);
    if (!i1 || !i2) return false;
    return hasLiveItemLink(i1, named_properties.spouse, q2)
        || hasLiveItemLink(i2, named_properties.spouse, q1);
}

export function isSiblingOf(q1, q2) {
    const wd = getWd();
    const i1 = wd.getItem(q1);
    const i2 = wd.getItem(q2);
    if (!i1 || !i2) return false;
    return hasLiveItemLink(i1, named_properties.sibling, q2)
        || hasLiveItemLink(i2, named_properties.sibling, q1);
}

// Transitive siblinghood (issue #81): two people who share a common sibling are
// themselves siblings, which lets relator2 deduce a sibling link even when no
// parents are recorded to tie them together (John–Henry and Henry–Frederick
// imply John–Frederick). Given the focus Q-id and a pool of loaded Q-ids,
// returns the pool members that share at least one sibling with the focus.
// Sibling links are read in both directions; the focus itself is excluded.
export function mutualSiblingsInPool(rootQ, pool) {
    const rootSiblings = pool.filter((q) => isSiblingOf(q, rootQ));
    return pool.filter((q) =>
        q !== rootQ && rootSiblings.some((s) => s !== q && isSiblingOf(s, q)));
}

export function isRelativeOf(q1, q2) {
    return q1 !== q2
        && !isSpouseOf(q1, q2)
        && !isAncestorOf(q1, q2)
        && !isAncestorOf(q2, q1);
}

// Build the `select-suggestion` event for the *mirror* of a suggestion when its
// selection is toggled, so both directions of the relation stay in sync.
// `nextSelected` is the suggestion's new selected state. Returns null when the
// target isn't loaded or the property has no usable (gendered) mirror — in
// which case only the suggestion's own selected flag changes. Shared by the
// person card's checkbox and the diagram layout's clickable suggestion lines.
export function mirrorSelectionEvent(suggestion, nextSelected) {
    if (!hasMirrorProperty(suggestion.prop, mirror_properties)) return null;
    const target = getWd().getItem(suggestion.to);
    if (target === undefined) return null;
    const mirrors = mirror_properties[suggestion.prop];
    const mp = mirrors.length === 1
        ? mirrors[0]
        : (target.hasClaimItemLink(named_properties.gender, named_items.female) ? mirrors[1] : mirrors[0]);
    if (mp === undefined) return null;
    return { from: suggestion.to, prop: mp, to: suggestion.from, suggestion: true, selected: nextSelected };
}

export function getGenderedMirrorProperty(item, mirrorProps) {
    if (mirrorProps.length === 1) return mirrorProps[0];
    if (item.hasClaimItemLink(named_properties.gender, named_items.male))   return mirrorProps[0];
    if (item.hasClaimItemLink(named_properties.gender, named_items.female)) return mirrorProps[1];
    return undefined;
}

function relationFromClaim(from, prop, claim) {
    if (isDeprecatedClaim(claim)) return undefined;
    const snaktype = claim?.mainsnak?.snaktype;
    if (snaktype === 'somevalue') return { from, prop, to: 'somevalue' };
    if (snaktype === 'novalue')   return { from, prop, to: 'novalue' };
    const value = claim?.mainsnak?.datavalue?.value;
    if (value?.['entity-type'] !== 'item') return undefined;
    return { from, prop, to: value.id };
}

export function relationClaims(item) {
    const this_q = item.getID();
    const claims = Object.keys(mirror_properties)
        .flatMap((prop) =>
            item.getClaimsForProperty(prop)
                .map((claim) => relationFromClaim(this_q, prop, claim))
                .filter((rel) => rel !== undefined));
    return sortRelations(claims);
}

// The relatives search (haswbstatement) matches statements of any rank, so it
// can surface an item whose only link to the focus person is deprecated. Such
// an item has no live relation tying it into the graph and would otherwise
// render as a stray card. Keep the focus item plus every item that appears as
// the source or an item target of some live relation; drop the rest.
export function connectedItems(items, itemRelations, mainQ) {
    const connected = new Set();
    if (mainQ) connected.add(mainQ);
    for (const q of items) {
        for (const rel of itemRelations[q] ?? []) {
            connected.add(rel.from);
            if (/^Q\d+$/.test(rel.to)) connected.add(rel.to);
        }
    }
    return items.filter((q) => connected.has(q));
}

export function recomputeItemRelations(updated_items) {
    const wd = getWd();
    const flat = Array.isArray(updated_items) ? updated_items.flat() : [];
    const targets = flat.length > 0 ? flat : appState.items;
    for (const q of targets) {
        const item = wd.getItem(q);
        if (item !== undefined) setItemRelations(item.getID(), relationClaims(item));
    }
}
