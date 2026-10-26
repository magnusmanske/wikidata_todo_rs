import * as utils from '../utils.js';

test('parseDate returns month 04 for "3.4.1900"', function () {
    assert.equal(utils.parseDate('3.4.1900'), '1900-04-03');
});

test('parseDate returns month 06 for "15.6.2020"', function () {
    assert.equal(utils.parseDate('15.6.2020'), '2020-06-15');
});

test('parseDate returns month 12 for "31.12.1999"', function () {
    assert.equal(utils.parseDate('31.12.1999'), '1999-12-31');
});
