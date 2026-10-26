import * as utils from '../utils.js';

test('parseDate returns empty string for non-date input', function () {
    assert.equal(utils.parseDate(''), '');
    assert.equal(utils.parseDate('unknown'), '');
    assert.equal(utils.parseDate('n/a'), '');
});
