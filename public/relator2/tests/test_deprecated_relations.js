if (typeof globalThis.Vue === 'undefined') {
    function VueStub() {}
    VueStub.observable = (o) => o;
    VueStub.set = (o, k, v) => { o[k] = v; };
    globalThis.Vue = VueStub;
}

const relations = await import('../relations.js');

function itemClaim(q, rank) {
    return {
        mainsnak: {
            snaktype: 'value',
            datavalue: { value: { 'entity-type': 'item', id: q }, type: 'wikibase-entityid' },
        },
        type: 'statement',
        rank: rank ?? 'normal',
    };
}

function fakeItem(id, claimsByProp) {
    return {
        getID: () => id,
        getClaimsForProperty: (p) => claimsByProp[p] ?? [],
    };
}

const { named_properties } = relations;

test('isDeprecatedClaim only flags the deprecated rank', function () {
    assert.equal(relations.isDeprecatedClaim(itemClaim('Q1', 'deprecated')), true);
    assert.equal(relations.isDeprecatedClaim(itemClaim('Q1', 'normal')), false);
    assert.equal(relations.isDeprecatedClaim(itemClaim('Q1', 'preferred')), false);
    // A missing rank defaults to normal on Wikidata, so it is not deprecated.
    assert.equal(relations.isDeprecatedClaim({ mainsnak: {} }), false);
});

test('liveRelationItems drops deprecated targets but keeps normal and preferred', function () {
    const item = fakeItem('Q100', {
        [named_properties.spouse]: [
            itemClaim('Q1', 'normal'),
            itemClaim('Q2', 'deprecated'),
            itemClaim('Q3', 'preferred'),
        ],
    });
    assert.deepEqual(relations.liveRelationItems(item, named_properties.spouse), ['Q1', 'Q3']);
});

test('liveRelationItems ignores somevalue/novalue snaks', function () {
    const item = fakeItem('Q100', {
        [named_properties.child]: [
            { mainsnak: { snaktype: 'somevalue' }, rank: 'normal' },
            { mainsnak: { snaktype: 'novalue' }, rank: 'normal' },
            itemClaim('Q5', 'normal'),
        ],
    });
    assert.deepEqual(relations.liveRelationItems(item, named_properties.child), ['Q5']);
});

test('relationClaims excludes deprecated relations from the relations table', function () {
    const item = fakeItem('Q100', {
        [named_properties.spouse]: [
            itemClaim('Q1', 'normal'),
            itemClaim('Q2', 'deprecated'),
        ],
    });
    const spouses = relationClaimsTargets(item, named_properties.spouse);
    assert.deepEqual(spouses, ['Q1']);
});

function relationClaimsTargets(item, prop) {
    return relations.relationClaims(item)
        .filter((r) => r.prop === prop)
        .map((r) => r.to);
}

test('connectedItems keeps the focus item even with no relations', function () {
    assert.deepEqual(relations.connectedItems(['Q100'], {}, 'Q100'), ['Q100']);
});

test('connectedItems drops a relative surfaced only by a deprecated link', function () {
    // Q200 (a deprecated-only spouse) was returned by the relatives search but
    // has no live relation, and the focus item's live relations never mention
    // it. It must be pruned; the live child Q300 stays.
    const items = ['Q100', 'Q200', 'Q300'];
    const itemRelations = {
        Q100: [{ from: 'Q100', prop: named_properties.child, to: 'Q300' }],
        Q200: [],
        Q300: [{ from: 'Q300', prop: named_properties.father, to: 'Q100' }],
    };
    assert.deepEqual(relations.connectedItems(items, itemRelations, 'Q100'), ['Q100', 'Q300']);
});

test('connectedItems keeps a relative linked to a non-focus relative', function () {
    // Q300 relates to Q200 (not the focus item) via a live statement, so it is
    // part of the live graph and must be kept.
    const items = ['Q100', 'Q200', 'Q300'];
    const itemRelations = {
        Q100: [{ from: 'Q100', prop: named_properties.spouse, to: 'Q200' }],
        Q200: [{ from: 'Q200', prop: named_properties.child, to: 'Q300' }],
        Q300: [],
    };
    assert.deepEqual(relations.connectedItems(items, itemRelations, 'Q100'), ['Q100', 'Q200', 'Q300']);
});

test('connectedItems ignores somevalue/novalue relation targets', function () {
    const items = ['Q100', 'Q200'];
    const itemRelations = {
        Q100: [{ from: 'Q100', prop: named_properties.father, to: 'somevalue' }],
        Q200: [],
    };
    assert.deepEqual(relations.connectedItems(items, itemRelations, 'Q100'), ['Q100']);
});
