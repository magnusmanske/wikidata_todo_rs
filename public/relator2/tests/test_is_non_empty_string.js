import * as utils from '../utils.js';

test('isNonEmptyString accepts non-empty strings', function () {
    assert.equal(utils.isNonEmptyString('hello'), true);
    assert.equal(utils.isNonEmptyString('0'), true);
    assert.equal(utils.isNonEmptyString('https://example.org'), true);
});

test('isNonEmptyString rejects empty string and nullish values', function () {
    assert.equal(utils.isNonEmptyString(''), false);
    assert.equal(utils.isNonEmptyString(undefined), false);
    assert.equal(utils.isNonEmptyString(null), false);
});

test('isNonEmptyString rejects non-string inputs', function () {
    assert.equal(utils.isNonEmptyString(0), false);
    assert.equal(utils.isNonEmptyString(1), false);
    assert.equal(utils.isNonEmptyString(false), false);
    assert.equal(utils.isNonEmptyString({}), false);
});
