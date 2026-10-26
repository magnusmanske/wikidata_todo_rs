import { relationTargetsToLoad } from '../utils.js';

const RELATION_PROPS = ['P22', 'P25', 'P26', 'P40'];

test('relationTargetsToLoad keeps relation-property targets', () => {
    const out = relationTargetsToLoad(
        [{ prop: 'P22', to: 'Q10' }, { prop: 'P26', to: 'Q11' }],
        [], RELATION_PROPS);
    assert.deepEqual(out, ['Q10', 'Q11']);
});

test('relationTargetsToLoad drops gender (P21) and drop-rule (P27) targets', () => {
    const out = relationTargetsToLoad(
        [{ prop: 'P21', to: 'Q6581097' }, { prop: 'P27', to: 'Q30' }, { prop: 'P40', to: 'Q12' }],
        [], RELATION_PROPS);
    assert.deepEqual(out, ['Q12']);
});

test('relationTargetsToLoad skips items already in view', () => {
    const out = relationTargetsToLoad(
        [{ prop: 'P22', to: 'Q10' }, { prop: 'P25', to: 'Q11' }],
        ['Q10'], RELATION_PROPS);
    assert.deepEqual(out, ['Q11']);
});

test('relationTargetsToLoad de-duplicates targets', () => {
    const out = relationTargetsToLoad(
        [{ prop: 'P22', to: 'Q10' }, { prop: 'P26', to: 'Q10' }],
        [], RELATION_PROPS);
    assert.deepEqual(out, ['Q10']);
});

test('relationTargetsToLoad returns empty when there are no statements', () => {
    assert.deepEqual(relationTargetsToLoad([], [], RELATION_PROPS), []);
});

test('relationTargetsToLoad returns empty when only non-relation statements are given', () => {
    const out = relationTargetsToLoad(
        [{ prop: 'P21', to: 'Q6581072' }],
        [], RELATION_PROPS);
    assert.deepEqual(out, []);
});
