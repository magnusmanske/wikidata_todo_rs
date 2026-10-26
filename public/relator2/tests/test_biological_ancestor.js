if (typeof globalThis.Vue === 'undefined') {
    function VueStub() {}
    VueStub.observable = (o) => o;
    VueStub.set = (o, k, v) => { o[k] = v; };
    globalThis.Vue = VueStub;
}

const relations = await import('../relations.js');
const { getWd } = await import('../app-state.js');

const { named_properties, isAncestorOf, isBiologicalAncestorOf } = relations;
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

test('isBiologicalAncestorOf accepts a father link', function () {
    setup({ Q1: { [np.father]: [itemLink('Q2')] }, Q2: {} });
    assert.equal(isBiologicalAncestorOf('Q2', 'Q1'), true);
});

test('isBiologicalAncestorOf accepts a mother link', function () {
    setup({ Q1: { [np.mother]: [itemLink('Q2')] }, Q2: {} });
    assert.equal(isBiologicalAncestorOf('Q2', 'Q1'), true);
});

test('isBiologicalAncestorOf accepts an explicit child link', function () {
    setup({ Q1: { [np.child]: [itemLink('Q2')] }, Q2: {} });
    assert.equal(isBiologicalAncestorOf('Q1', 'Q2'), true);
});

test('isBiologicalAncestorOf rejects a stepparent link (issue #79)', function () {
    // The child Q1 has Q2 as a step-parent only. isAncestorOf still treats this
    // as ancestry (for tree layout), but the biological check must not — so the
    // step-child is never offered as a child.
    setup({ Q1: { [np.stepparent]: [itemLink('Q2')] }, Q2: {} });
    assert.equal(isAncestorOf('Q2', 'Q1'), true);
    assert.equal(isBiologicalAncestorOf('Q2', 'Q1'), false);
});

test('isBiologicalAncestorOf rejects a godparent link', function () {
    setup({ Q1: { [np.godparent]: [itemLink('Q2')] }, Q2: {} });
    assert.equal(isAncestorOf('Q2', 'Q1'), true);
    assert.equal(isBiologicalAncestorOf('Q2', 'Q1'), false);
});

test('isBiologicalAncestorOf ignores a deprecated father link', function () {
    const dep = { ...itemLink('Q2'), rank: 'deprecated' };
    setup({ Q1: { [np.father]: [dep] }, Q2: {} });
    assert.equal(isBiologicalAncestorOf('Q2', 'Q1'), false);
});

test('isBiologicalAncestorOf returns false for unrelated people', function () {
    setup({ Q1: {}, Q2: {} });
    assert.equal(isBiologicalAncestorOf('Q1', 'Q2'), false);
});
