import * as utils from '../utils.js';

test('parseDate returns month 02 for "1. 2. 1934"', function () {
    assert.equal(utils.parseDate('1. 2. 1934'), '1934-02-01');
});

test('parseDate returns month 12 for "5. 12. 2001"', function () {
    assert.equal(utils.parseDate('5. 12. 2001'), '2001-12-05');
});

test('parseDate still parses named German months', function () {
    assert.equal(utils.parseDate('1. Januar 1900'), '1900-01-01');
    assert.equal(utils.parseDate('15. März 1827'), '1827-03-15');
});
