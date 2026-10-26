if (typeof globalThis.Vue === 'undefined') {
    function VueStub() {}
    VueStub.observable = (o) => o;
    VueStub.set = (o, k, v) => { o[k] = v; };
    globalThis.Vue = VueStub;
}

const { isImpossibleRelation } = await import('../sanity.js');
const { named_properties } = await import('../relations.js');
const { getWd } = await import('../app-state.js');

const np = named_properties;

function itemLink(q) {
    return { mainsnak: { snaktype: 'value', datavalue: { value: { 'entity-type': 'item', id: q }, type: 'wikibase-entityid' } }, rank: 'normal' };
}
function dateClaim(time, precision) {
    return { mainsnak: { snaktype: 'value', datavalue: { value: { time, precision }, type: 'time' } }, rank: 'normal' };
}
const pad = (n, w) => String(n).padStart(w, '0');
function year(y)         { return dateClaim(`+${pad(y, 4)}-00-00T00:00:00Z`, 9); }
function day(y, m, d)    { return dateClaim(`+${pad(y, 4)}-${pad(m, 2)}-${pad(d, 2)}T00:00:00Z`, 11); }
function century(y)      { return dateClaim(`+${pad(y, 4)}-00-00T00:00:00Z`, 7); }
function decade(y)       { return dateClaim(`+${pad(y, 4)}-00-00T00:00:00Z`, 8); }

function fakeItem(id, claimsByProp) {
    return {
        getID: () => id,
        getLabel: () => id,
        getClaimsForProperty: (p) => claimsByProp[p] ?? [],
        hasClaimItemLink: (p, q) => (claimsByProp[p] ?? []).some((c) => c.mainsnak?.datavalue?.value?.id === q),
    };
}

function setup(items) {
    getWd().items = {};
    for (const [id, claims] of Object.entries(items)) getWd().items[id] = fakeItem(id, claims);
}

const impossible = (rel) => isImpossibleRelation(rel, getWd());

test('a mother who died years before the birth is impossible', function () {
    setup({
        Q2: { [np.death_date]: [year(1800)] },      // alleged mother
        Q1: { [np.birth_date]: [year(1805)] },      // child
    });
    assert.ok(impossible({ from: 'Q1', prop: np.mother, to: 'Q2' }));
});

test('a mother dying in the year before the birth is still offered', function () {
    // Flagged as a warning at save time, but within the suppression grace year:
    // year precision leaves the intervals nearly adjacent, so keep showing it.
    setup({
        Q2: { [np.death_date]: [year(1800)] },
        Q1: { [np.birth_date]: [year(1801)] },
    });
    assert.notOk(impossible({ from: 'Q1', prop: np.mother, to: 'Q2' }));
});

test('a father dead well over a year before the birth is impossible', function () {
    setup({
        Q2: { [np.death_date]: [year(1800)] },
        Q1: { [np.birth_date]: [year(1803)] },
    });
    assert.ok(impossible({ from: 'Q1', prop: np.father, to: 'Q2' }));
});

test('a posthumous child of a father dead two years earlier is still offered', function () {
    // The father rule already grants a grace year; the suppression grace adds a
    // second one, so a 1800 death and an 1802 birth stays visible.
    setup({
        Q2: { [np.death_date]: [year(1800)] },
        Q1: { [np.birth_date]: [year(1802)] },
    });
    assert.notOk(impossible({ from: 'Q1', prop: np.father, to: 'Q2' }));
});

test('day precision against a century never suppresses', function () {
    // Q2 died "19th century", Q1 born 1805-03-02: the century is far too vague
    // to contradict anything, so the relation must stay visible.
    setup({
        Q2: { [np.death_date]: [century(1800)] },
        Q1: { [np.birth_date]: [day(1805, 3, 2)] },
    });
    assert.notOk(impossible({ from: 'Q1', prop: np.mother, to: 'Q2' }));
});

test('day precision against a decade never suppresses', function () {
    setup({
        Q2: { [np.death_date]: [decade(1790)] },
        Q1: { [np.birth_date]: [day(1805, 3, 2)] },
    });
    assert.notOk(impossible({ from: 'Q1', prop: np.mother, to: 'Q2' }));
});

test('day-precision dates a few months apart never suppress', function () {
    setup({
        Q2: { [np.death_date]: [day(1800, 1, 10)] },
        Q1: { [np.birth_date]: [day(1800, 9, 30)] },
    });
    assert.notOk(impossible({ from: 'Q1', prop: np.mother, to: 'Q2' }));
});

test('day-precision dates two years apart do suppress', function () {
    setup({
        Q2: { [np.death_date]: [day(1800, 1, 10)] },
        Q1: { [np.birth_date]: [day(1802, 9, 30)] },
    });
    assert.ok(impossible({ from: 'Q1', prop: np.mother, to: 'Q2' }));
});

test('a P40 child suggestion reads the parent sex for the grace period', function () {
    setup({
        Q2: { [np.gender]: [itemLink('Q6581072')], [np.death_date]: [year(1800)] }, // female
        Q1: { [np.birth_date]: [year(1805)] },
    });
    assert.ok(impossible({ from: 'Q2', prop: np.child, to: 'Q1' }));
});

test('a parent born long after the child is impossible', function () {
    setup({
        Q2: { [np.birth_date]: [year(1850)] },
        Q1: { [np.birth_date]: [year(1800)] },
    });
    assert.ok(impossible({ from: 'Q1', prop: np.father, to: 'Q2' }));
});

test('a spouse suggestion with no lifespan overlap is impossible', function () {
    setup({
        Q1: { [np.birth_date]: [year(1700)], [np.death_date]: [year(1750)] },
        Q2: { [np.birth_date]: [year(1800)] },
    });
    assert.ok(impossible({ from: 'Q1', prop: np.spouse, to: 'Q2' }));
});

test('a plausible relation is never suppressed', function () {
    setup({
        Q2: { [np.birth_date]: [year(1770)], [np.death_date]: [year(1840)] },
        Q1: { [np.birth_date]: [year(1800)] },
    });
    assert.notOk(impossible({ from: 'Q1', prop: np.mother, to: 'Q2' }));
});

test('a relation with no dates at all is never suppressed', function () {
    setup({ Q1: {}, Q2: {} });
    assert.notOk(impossible({ from: 'Q1', prop: np.mother, to: 'Q2' }));
});

test('structural red flags are warnings, not grounds for suppression', function () {
    // Married siblings are a data smell worth flagging, but dynastic marriages
    // happened — the suggestion must still be shown.
    setup({ Q1: { [np.sibling]: [itemLink('Q2')] }, Q2: {} });
    assert.notOk(impossible({ from: 'Q1', prop: np.spouse, to: 'Q2' }));
});
