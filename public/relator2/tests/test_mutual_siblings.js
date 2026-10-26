if (typeof globalThis.Vue === 'undefined') {
    function VueStub() {}
    VueStub.observable = (o) => o;
    VueStub.set = (o, k, v) => { o[k] = v; };
    globalThis.Vue = VueStub;
}

const relations = await import('../relations.js');
const { getWd } = await import('../app-state.js');

const { named_properties, mutualSiblingsInPool } = relations;
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

test('mutualSiblingsInPool deduces a sibling through a shared sibling (issue #81)', function () {
    // John (Q1)–Henry (Q2) are siblings, and Henry (Q2)–Frederick (Q3) are
    // siblings, so John and Frederick should be deduced as siblings too.
    setup({
        Q1: { [np.sibling]: [itemLink('Q2')] },
        Q2: { [np.sibling]: [itemLink('Q1'), itemLink('Q3')] },
        Q3: { [np.sibling]: [itemLink('Q2')] },
    });
    assert.deepEqual(mutualSiblingsInPool('Q1', ['Q1', 'Q2', 'Q3']), ['Q3']);
});

test('mutualSiblingsInPool follows sibling links in both directions', function () {
    // Only Henry (Q2) records the sibling links; John and Frederick are still
    // tied together through him.
    setup({
        Q1: {},
        Q2: { [np.sibling]: [itemLink('Q1'), itemLink('Q3')] },
        Q3: {},
    });
    assert.deepEqual(mutualSiblingsInPool('Q1', ['Q1', 'Q2', 'Q3']), ['Q3']);
});

test('mutualSiblingsInPool never returns the focus person', function () {
    setup({
        Q1: { [np.sibling]: [itemLink('Q2')] },
        Q2: { [np.sibling]: [itemLink('Q1')] },
    });
    assert.deepEqual(mutualSiblingsInPool('Q1', ['Q1', 'Q2']), []);
});

test('mutualSiblingsInPool returns nothing without a shared sibling', function () {
    // A bare direct sibling (no third sibling in common) is handled elsewhere,
    // so the transitive pass returns nothing here.
    setup({
        Q1: { [np.sibling]: [itemLink('Q2')] },
        Q2: { [np.sibling]: [itemLink('Q1')] },
        Q3: {},
    });
    assert.deepEqual(mutualSiblingsInPool('Q1', ['Q1', 'Q2', 'Q3']), []);
});

test('mutualSiblingsInPool ignores a deprecated sibling link', function () {
    const dep = { ...itemLink('Q3'), rank: 'deprecated' };
    setup({
        Q1: { [np.sibling]: [itemLink('Q2')] },
        Q2: { [np.sibling]: [itemLink('Q1'), dep] },
        Q3: {},
    });
    assert.deepEqual(mutualSiblingsInPool('Q1', ['Q1', 'Q2', 'Q3']), []);
});
