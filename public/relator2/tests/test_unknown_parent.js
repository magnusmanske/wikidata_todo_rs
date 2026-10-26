if (typeof globalThis.Vue === 'undefined') {
    function VueStub() {}
    VueStub.observable = (o) => o;
    VueStub.set = (o, k, v) => { o[k] = v; };
    globalThis.Vue = VueStub;
}

const { named_properties: np, hasUnknownParent } = await import('../relations.js');

function snak(snaktype, rank = 'normal') {
    return { mainsnak: { snaktype }, rank };
}
function itemLink(q) {
    return { mainsnak: { snaktype: 'value', datavalue: { value: { 'entity-type': 'item', id: q }, type: 'wikibase-entityid' } }, rank: 'normal' };
}
function fakeItem(claimsByProp) {
    return { getClaimsForProperty: (p) => claimsByProp[p] ?? [] };
}

test('an "unknown value" father blocks father suggestions (Q141541332)', function () {
    const item = fakeItem({ [np.father]: [snak('somevalue')], [np.mother]: [itemLink('Q2')] });
    assert.equal(hasUnknownParent(item, np.father), true);
    assert.equal(hasUnknownParent(item, np.mother), false);
});

test('a "no value" mother blocks mother suggestions', function () {
    assert.equal(hasUnknownParent(fakeItem({ [np.mother]: [snak('novalue')] }), np.mother), true);
});

test('a deprecated "unknown value" parent does not block suggestions', function () {
    assert.equal(hasUnknownParent(fakeItem({ [np.father]: [snak('somevalue', 'deprecated')] }), np.father), false);
});

test('an "unknown value" only blocks the single-valued parent properties', function () {
    assert.equal(hasUnknownParent(fakeItem({ [np.spouse]: [snak('somevalue')] }), np.spouse), false);
});

test('a missing item never blocks suggestions', function () {
    assert.equal(hasUnknownParent(undefined, np.father), false);
});
