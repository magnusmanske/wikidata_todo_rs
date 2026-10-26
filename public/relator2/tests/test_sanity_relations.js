if (typeof globalThis.Vue === 'undefined') {
    function VueStub() {}
    VueStub.observable = (o) => o;
    VueStub.set = (o, k, v) => { o[k] = v; };
    globalThis.Vue = VueStub;
}

const sanity = await import('../sanity.js');
const { relationWarnings } = sanity;
const { named_properties, named_items } = await import('../relations.js');
const { getWd } = await import('../app-state.js');

const np = named_properties;

function itemLink(q) {
    return { mainsnak: { snaktype: 'value', datavalue: { value: { 'entity-type': 'item', id: q }, type: 'wikibase-entityid' } }, rank: 'normal' };
}
function dateClaim(time, precision) {
    return { mainsnak: { snaktype: 'value', datavalue: { value: { time, precision }, type: 'time' } }, rank: 'normal' };
}
function year(y) { return dateClaim(`+${String(y).padStart(4, '0')}-00-00T00:00:00Z`, 9); }

function fakeItem(id, claimsByProp) {
    return {
        getID: () => id,
        getLabel: () => id,
        getClaimsForProperty: (p) => claimsByProp[p] ?? [],
        hasClaimItemLink: (p, q) => (claimsByProp[p] ?? []).some((c) => c.mainsnak?.datavalue?.value?.id === q),
    };
}

// Replace the shared store's items with a fresh fixture for one test.
function setup(items) {
    getWd().items = {};
    for (const [id, claims] of Object.entries(items)) getWd().items[id] = fakeItem(id, claims);
}

const codes = (rel, opts) => relationWarnings(rel, getWd(), opts).map((w) => w.code).sort();

test('a self-relation is rejected outright', function () {
    setup({ Q1: {} });
    assert.deepEqual(codes({ from: 'Q1', prop: np.spouse, to: 'Q1' }), ['self']);
});

test('siblings cannot be flagged as a couple', function () {
    setup({ Q1: { [np.sibling]: [itemLink('Q2')] }, Q2: {} });
    assert.deepEqual(codes({ from: 'Q1', prop: np.spouse, to: 'Q2' }), ['couple_siblings']);
});

test('a parent and child cannot be flagged as a couple', function () {
    // Q1's father is Q2, so Q2 is an ancestor of Q1.
    setup({ Q1: { [np.father]: [itemLink('Q2')] }, Q2: {} });
    assert.deepEqual(codes({ from: 'Q1', prop: np.unmarried_partner, to: 'Q2' }), ['couple_parent_child']);
});

test('adding a blood relation between known spouses warns (the reported bug)', function () {
    // Q1 is married to Q2; marking Q2 as Q1's mother should be flagged.
    setup({ Q1: { [np.spouse]: [itemLink('Q2')] }, Q2: {} });
    assert.deepEqual(codes({ from: 'Q1', prop: np.mother, to: 'Q2' }), ['blood_couple']);
});

test('spouses whose lifespans never overlap are flagged', function () {
    setup({
        Q1: { [np.birth_date]: [year(1700)], [np.death_date]: [year(1750)] },
        Q2: { [np.birth_date]: [year(1800)] },
    });
    assert.deepEqual(codes({ from: 'Q1', prop: np.spouse, to: 'Q2' }), ['couple_no_overlap']);
});

test('a child born well after the father died is flagged', function () {
    setup({
        Q2: { [np.death_date]: [year(1800)] },          // father
        Q1: { [np.birth_date]: [year(1805)] },          // child
    });
    assert.deepEqual(codes({ from: 'Q1', prop: np.father, to: 'Q2' }), ['child_after_parent_death']);
});

test('a mother dying before the birth is flagged without a grace year', function () {
    setup({
        Q2: { [np.death_date]: [year(1800)] },          // mother
        Q1: { [np.birth_date]: [year(1801)] },          // child, year after
    });
    assert.deepEqual(codes({ from: 'Q1', prop: np.mother, to: 'Q2' }), ['child_after_parent_death']);
});

test('a parent born after the child is flagged', function () {
    setup({
        Q2: { [np.birth_date]: [year(1850)] },          // alleged father
        Q1: { [np.birth_date]: [year(1800)] },          // child
    });
    assert.deepEqual(codes({ from: 'Q1', prop: np.father, to: 'Q2' }), ['parent_after_child']);
});

test('P40 reads the grace year from the parent sex', function () {
    setup({
        Q2: { [np.gender]: [itemLink(named_items.female)], [np.death_date]: [year(1800)] },
        Q1: { [np.birth_date]: [year(1801)] },
    });
    // Q2 (female parent) → mother rule, no grace → flagged.
    assert.deepEqual(codes({ from: 'Q2', prop: np.child, to: 'Q1' }), ['child_after_parent_death']);
});

test('a plausible marriage produces no warnings', function () {
    setup({
        Q1: { [np.birth_date]: [year(1800)], [np.death_date]: [year(1860)] },
        Q2: { [np.birth_date]: [year(1810)], [np.death_date]: [year(1870)] },
    });
    assert.deepEqual(codes({ from: 'Q1', prop: np.spouse, to: 'Q2' }), []);
});

test('date overrides validate a not-yet-created person', function () {
    setup({ Q2: { [np.birth_date]: [year(1900)] } });
    const opts = {
        dateOverrides: { __new__: { death: { lo: 1850, hi: 1851 } } },
        labels: { __new__: 'New Person' },
    };
    assert.deepEqual(codes({ from: '__new__', prop: np.spouse, to: 'Q2' }, opts), ['couple_no_overlap']);
});

test('a known parent cannot also be added as a sibling (the reported bug)', function () {
    // Q1's father is Q2; marking Q2 as Q1's sibling should be flagged.
    setup({ Q1: { [np.father]: [itemLink('Q2')] }, Q2: {} });
    assert.deepEqual(codes({ from: 'Q1', prop: np.sibling, to: 'Q2' }), ['sibling_parent_child']);
});

test('the sibling/parent conflict is caught from the parent side too', function () {
    // Same data, but the mirror sibling edit written on the parent's item.
    setup({ Q1: { [np.father]: [itemLink('Q2')] }, Q2: {} });
    assert.deepEqual(codes({ from: 'Q2', prop: np.sibling, to: 'Q1' }), ['sibling_parent_child']);
});

test('a known sibling cannot also be added as a parent', function () {
    // Q1 and Q2 are siblings; marking Q2 as Q1's father should be flagged.
    setup({ Q1: { [np.sibling]: [itemLink('Q2')] }, Q2: {} });
    assert.deepEqual(codes({ from: 'Q1', prop: np.father, to: 'Q2' }), ['parent_child_sibling']);
});

test('a known sibling cannot also be added as a child', function () {
    setup({ Q1: { [np.sibling]: [itemLink('Q2')] }, Q2: {} });
    assert.deepEqual(codes({ from: 'Q1', prop: np.child, to: 'Q2' }), ['parent_child_sibling']);
});

test('a godparent may also be a sibling (no false positive)', function () {
    // Godparent is not a biological parent, so siblinghood must not be flagged.
    setup({ Q1: { [np.godparent]: [itemLink('Q2')] }, Q2: {} });
    assert.deepEqual(codes({ from: 'Q1', prop: np.sibling, to: 'Q2' }), []);
});

test('non-kinship properties are never flagged', function () {
    setup({ Q1: { [np.spouse]: [itemLink('Q2')] }, Q2: {} });
    assert.deepEqual(codes({ from: 'Q1', prop: np.relative, to: 'Q2' }), []);
});
