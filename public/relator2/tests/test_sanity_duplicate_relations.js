if (typeof globalThis.Vue === 'undefined') {
    function VueStub() {}
    VueStub.observable = (o) => o;
    VueStub.set = (o, k, v) => { o[k] = v; };
    globalThis.Vue = VueStub;
}

const { duplicateRelationTargets, itemRelationWarnings } = await import('../sanity.js');
const { named_properties } = await import('../relations.js');
const { getWd } = await import('../app-state.js');

const np = named_properties;

function fakeItem(id) {
    return { getID: () => id, getLabel: () => id };
}
function setup(ids) {
    getWd().items = {};
    for (const id of ids) getWd().items[id] = fakeItem(id);
}

const rel = (prop, to, from = 'Q1') => ({ from, prop, to });
const targets = (relations) => [...duplicateRelationTargets(relations)].sort();
const codes = (relations) => itemRelationWarnings(relations, getWd()).map((w) => w.code);

test('the same person as spouse twice is flagged', function () {
    setup(['Q1', 'Q2']);
    assert.deepEqual(targets([rel(np.spouse, 'Q2'), rel(np.spouse, 'Q2')]), ['Q2']);
    assert.deepEqual(codes([rel(np.spouse, 'Q2'), rel(np.spouse, 'Q2')]), ['duplicate_person']);
});

test('the same person as spouse and sibling is flagged', function () {
    setup(['Q1', 'Q2']);
    assert.deepEqual(targets([rel(np.spouse, 'Q2'), rel(np.sibling, 'Q2')]), ['Q2']);
});

test('distinct people in distinct relations are not flagged', function () {
    setup(['Q1', 'Q2', 'Q3']);
    assert.deepEqual(targets([rel(np.spouse, 'Q2'), rel(np.sibling, 'Q3')]), []);
});

test('teacher/student links to the same person are exempt', function () {
    setup(['Q1', 'Q2']);
    assert.deepEqual(targets([rel(np.student_of, 'Q2'), rel(np.student, 'Q2')]), []);
});

test('a teacher/student link does not pair up with a kinship link', function () {
    // Q2 is both the teacher and a sibling — only the sibling counts, so the
    // pair never reaches two and is not flagged.
    setup(['Q1', 'Q2']);
    assert.deepEqual(targets([rel(np.student_of, 'Q2'), rel(np.sibling, 'Q2')]), []);
});

test('somevalue/novalue targets are ignored', function () {
    setup(['Q1']);
    assert.deepEqual(targets([rel(np.spouse, 'somevalue'), rel(np.sibling, 'novalue')]), []);
});

test('the warning names both people', function () {
    setup(['Q1', 'Q2']);
    const [w] = itemRelationWarnings([rel(np.spouse, 'Q2'), rel(np.child, 'Q2')], getWd());
    assert.equal(w.target, 'Q2');
    assert.ok(w.message.includes('Q1') && w.message.includes('Q2'));
});
