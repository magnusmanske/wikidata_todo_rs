if (typeof globalThis.Vue === 'undefined') {
    function VueStub() {}
    VueStub.observable = (o) => o;
    VueStub.set = (o, k, v) => { o[k] = v; };
    globalThis.Vue = VueStub;
}

const relations = await import('../relations.js');

test('named_properties includes student_of (P1066) and student (P802)', function () {
    assert.equal(relations.named_properties.student_of, 'P1066');
    assert.equal(relations.named_properties.student,    'P802');
});

test('existing named_properties remain stable', function () {
    assert.equal(relations.named_properties.father, 'P22');
    assert.equal(relations.named_properties.mother, 'P25');
    assert.equal(relations.named_properties.child,  'P40');
});

test('relation_properties includes the new pair without dropping existing entries', function () {
    const props = relations.relation_properties;
    assert.ok(props.includes('P1066'), 'relation_properties should include P1066');
    assert.ok(props.includes('P802'),  'relation_properties should include P802');
    for (const p of ['P22', 'P25', 'P1290', 'P3448', 'P40', 'P26', 'P3373', 'P1038', 'P451']) {
        assert.ok(props.includes(p), `relation_properties should still include ${p}`);
    }
});

test('mirror_properties maps P1066 <-> P802 as inverses', function () {
    assert.deepEqual(relations.mirror_properties.P1066, ['P802']);
    assert.deepEqual(relations.mirror_properties.P802,  ['P1066']);
});

test('existing mirror_properties pairings remain stable', function () {
    assert.deepEqual(relations.mirror_properties.P22, ['P40']);
    assert.deepEqual(relations.mirror_properties.P25, ['P40']);
    assert.deepEqual(relations.mirror_properties.P40, ['P22', 'P25']);
    assert.deepEqual(relations.mirror_properties.P26, ['P26']);
    assert.deepEqual(relations.mirror_properties.P3373, ['P3373']);
});

test('stepparent (P3448) and godparent (P1290) are keys with no reciprocal property', function () {
    // They must be present so the relation is displayed and selectable, but
    // map to an empty list because Wikidata has no stepchild/godchild property
    // (issue #78). hasMirrorProperty must still see them as known.
    assert.deepEqual(relations.mirror_properties.P3448, []);
    assert.deepEqual(relations.mirror_properties.P1290, []);
});

test('significant person (P3342) is a self-mirroring relation listed last', function () {
    assert.equal(relations.named_properties.significant_person, 'P3342');
    const props = relations.relation_properties;
    assert.ok(props.includes('P3342'), 'relation_properties should include P3342');
    assert.equal(props[props.length - 1], 'P3342', 'P3342 should be offered at the bottom');
    assert.deepEqual(relations.mirror_properties.P3342, ['P3342']);
});

test('maybe_default_properties covers the loose associations relative and significant person', function () {
    assert.ok(relations.maybe_default_properties.has('P1038'), 'relative (P1038) defaults to maybe');
    assert.ok(relations.maybe_default_properties.has('P3342'), 'significant person (P3342) defaults to maybe');
    // Committed relations must not be loosened to a "maybe" default.
    for (const p of ['P22', 'P25', 'P40', 'P26', 'P3373']) {
        assert.ok(!relations.maybe_default_properties.has(p), `${p} should not default to maybe`);
    }
});

test('getGenderedMirrorProperty yields no property for an empty mirror list', function () {
    const male   = { hasClaimItemLink: (p, q) => q === relations.named_items.male };
    const female = { hasClaimItemLink: (p, q) => q === relations.named_items.female };
    const none   = { hasClaimItemLink: () => false };
    assert.equal(relations.getGenderedMirrorProperty(male,   []), undefined);
    assert.equal(relations.getGenderedMirrorProperty(female, []), undefined);
    assert.equal(relations.getGenderedMirrorProperty(none,   []), undefined);
});

test('getGenderedMirrorProperty returns the single inverse for non-gendered pairs', function () {
    const fakeItem = { hasClaimItemLink: () => false };
    assert.equal(relations.getGenderedMirrorProperty(fakeItem, ['P802']),  'P802');
    assert.equal(relations.getGenderedMirrorProperty(fakeItem, ['P1066']), 'P1066');
});
