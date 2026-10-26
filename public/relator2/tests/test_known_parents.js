if (typeof globalThis.Vue === 'undefined') {
    function VueStub() {}
    VueStub.observable = (o) => o;
    VueStub.set = (o, k, v) => { o[k] = v; };
    globalThis.Vue = VueStub;
}

const relations = await import('../relations.js');
const { getWd } = await import('../app-state.js');

const { named_properties, knownParents, isRelativeOf } = relations;
const np = named_properties;

function itemLink(q) {
    return { mainsnak: { snaktype: 'value', datavalue: { value: { 'entity-type': 'item', id: q }, type: 'wikibase-entityid' } }, rank: 'normal' };
}
function fakeItem(id, claimsByProp) {
    return {
        getID: () => id,
        getClaimsForProperty: (p) => claimsByProp[p] ?? [],
        hasClaimItemLink: (p, q) => (claimsByProp[p] ?? []).some((c) => c.mainsnak?.datavalue?.value?.id === q),
    };
}
function setup(items) {
    getWd().items = {};
    for (const [id, claims] of Object.entries(items)) getWd().items[id] = fakeItem(id, claims);
}

test('knownParents lists every kind of parent link', function () {
    const item = fakeItem('Q1', {
        [np.father]:     [itemLink('Q2')],
        [np.mother]:     [itemLink('Q3')],
        [np.stepparent]: [itemLink('Q4')],
        [np.godparent]:  [itemLink('Q5')],
    });
    assert.deepEqual(knownParents(item).sort(), ['Q2', 'Q3', 'Q4', 'Q5']);
});

test('knownParents ignores a deprecated parent link', function () {
    const dep = { ...itemLink('Q9'), rank: 'deprecated' };
    const item = fakeItem('Q1', { [np.stepparent]: [dep] });
    assert.deepEqual(knownParents(item), []);
});

test('knownParents flags a stepparent so they are not re-suggested as father (issue #79)', function () {
    // George (Q2) is Marian's (Q1) explicit stepparent. Even though he is married
    // to her mother, the parent-suggestion code filters against knownParents, so
    // he must appear here and therefore never be offered as her father.
    const marian = fakeItem('Q1', { [np.stepparent]: [itemLink('Q2')] });
    assert.equal(knownParents(marian).includes('Q2'), true);
});

test('a step-relationship blocks the reciprocal "relative" mirror (issue #79)', function () {
    // George (Q2) has an explicit P1038 -> Marian (Q1), and Marian has George as a
    // stepparent. The relative mirror is gated on isRelativeOf, which is false for
    // a step-ancestor pair, so the reciprocal P1038 is never suggested on Marian.
    setup({
        Q1: { [np.stepparent]: [itemLink('Q2')] },
        Q2: { [np.relative]:   [itemLink('Q1')] },
    });
    assert.equal(isRelativeOf('Q1', 'Q2'), false);
});

test('genuine relatives still mirror as relatives', function () {
    setup({ Q1: {}, Q2: { [np.relative]: [itemLink('Q1')] } });
    assert.equal(isRelativeOf('Q1', 'Q2'), true);
});
