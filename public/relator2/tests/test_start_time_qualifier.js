import * as utils from '../utils.js';

test('startTimeQualifier: builds a well-formed P580 time qualifier', () => {
    const q = utils.startTimeQualifier('1850-06-15');
    assert.ok(q, 'qualifier object exists');
    assert.ok(Array.isArray(q.P580), 'P580 is an array');
    assert.equal(q.P580.length, 1);
    const snak = q.P580[0];
    assert.equal(snak.snaktype, 'value');
    assert.equal(snak.property, 'P580');
    assert.equal(snak.datatype, 'time');
    assert.equal(snak.datavalue.type, 'time');
    assert.equal(snak.datavalue.value.time, '+1850-06-15T00:00:00Z');
    assert.equal(snak.datavalue.value.precision, 11);
});

test('startTimeQualifier: year only keeps precision 9', () => {
    const q = utils.startTimeQualifier('1850');
    assert.equal(q.P580[0].datavalue.value.precision, 9);
    assert.equal(q.P580[0].datavalue.value.time, '+1850-01-01T00:00:00Z');
});

test('startTimeQualifier: invalid / empty → undefined', () => {
    assert.equal(utils.startTimeQualifier(''), undefined);
    assert.equal(utils.startTimeQualifier('xyz'), undefined);
    assert.equal(utils.startTimeQualifier(null), undefined);
});
