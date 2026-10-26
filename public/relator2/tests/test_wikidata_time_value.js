import * as utils from '../utils.js';

const CAL = 'http://www.wikidata.org/entity/Q1985727';

test('wikidataTimeValue: full ISO date → precision 11', () => {
    assert.deepEqual(utils.wikidataTimeValue('1850-06-15'), {
        time: '+1850-06-15T00:00:00Z',
        timezone: 0, before: 0, after: 0, precision: 11, calendarmodel: CAL,
    });
});

test('wikidataTimeValue: year-month → precision 10, day padded to 01', () => {
    assert.deepEqual(utils.wikidataTimeValue('1850-6'), {
        time: '+1850-06-01T00:00:00Z',
        timezone: 0, before: 0, after: 0, precision: 10, calendarmodel: CAL,
    });
});

test('wikidataTimeValue: year only → precision 9', () => {
    assert.deepEqual(utils.wikidataTimeValue('1850'), {
        time: '+1850-01-01T00:00:00Z',
        timezone: 0, before: 0, after: 0, precision: 9, calendarmodel: CAL,
    });
});

test('wikidataTimeValue: invalid / empty / non-string → undefined', () => {
    assert.equal(utils.wikidataTimeValue('not a date'), undefined);
    assert.equal(utils.wikidataTimeValue(''), undefined);
    assert.equal(utils.wikidataTimeValue(null), undefined);
    assert.equal(utils.wikidataTimeValue(1850), undefined);
});
