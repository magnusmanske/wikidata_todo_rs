import * as utils from '../utils.js';

test('parseDate returns empty string for null', function () {
    assert.equal(utils.parseDate(null), '');
});

test('parseDate returns empty string for undefined', function () {
    assert.equal(utils.parseDate(undefined), '');
});

test('parseDate returns empty string for non-string inputs', function () {
    assert.equal(utils.parseDate(42), '');
    assert.equal(utils.parseDate({}), '');
    assert.equal(utils.parseDate([]), '');
    assert.equal(utils.parseDate(true), '');
});
