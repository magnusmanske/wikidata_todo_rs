import { isSaveableDescription } from '../utils.js';

test('isSaveableDescription rejects an empty draft', () => {
    assert.notOk(isSaveableDescription('', 'old'));
});

test('isSaveableDescription rejects a whitespace-only draft', () => {
    assert.notOk(isSaveableDescription('   ', ''));
});

test('isSaveableDescription rejects a non-string draft', () => {
    assert.notOk(isSaveableDescription(undefined, ''));
    assert.notOk(isSaveableDescription(null, ''));
});

test('isSaveableDescription rejects a draft equal to the current value', () => {
    assert.notOk(isSaveableDescription('physicist', 'physicist'));
});

test('isSaveableDescription rejects a draft equal modulo surrounding whitespace', () => {
    assert.notOk(isSaveableDescription('  physicist  ', 'physicist'));
});

test('isSaveableDescription accepts a non-empty draft that differs from current', () => {
    assert.ok(isSaveableDescription('physicist', 'chemist'));
});

test('isSaveableDescription accepts adding a description where none exists', () => {
    assert.ok(isSaveableDescription('physicist', ''));
    assert.ok(isSaveableDescription('physicist', undefined));
});
