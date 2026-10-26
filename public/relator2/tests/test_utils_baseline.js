import * as utils from '../utils.js';

test('parseDate recognises ISO yyyy-mm-dd', function () {
    assert.equal(utils.parseDate('1927-03-15'), '1927-03-15');
});

test('parseDate falls back to plain year', function () {
    assert.equal(utils.parseDate('circa 1812'), '1812');
});

test('parseDate handles German "1. Januar 1900"', function () {
    assert.equal(utils.parseDate('1. Januar 1900'), '1900-01-01');
});

test('parseDate returns bare year when day is out of range', function () {
    assert.equal(utils.parseDate('99. Januar 1900'), '1900');
});

test('monthNameToNumber handles English and German variants', function () {
    assert.equal(utils.monthNameToNumber('January'), 1);
    assert.equal(utils.monthNameToNumber('März'), 3);
    assert.equal(utils.monthNameToNumber('Mai'), 5);
    assert.equal(utils.monthNameToNumber('Okt'), 10);
    assert.equal(utils.monthNameToNumber('Dez'), 12);
});

test('toWikidataDate pads all components', function () {
    assert.equal(utils.toWikidataDate('2005-04-07T00:00:00Z'), '+2005-04-07T00:00:00Z');
});

test('isValidURL accepts http(s) URLs', function () {
    assert.ok(utils.isValidURL('https://example.org/foo'));
    assert.ok(utils.isValidURL('http://wiki.example.com'));
});

test('numericSortItems sorts Q-ids by numeric part', function () {
    const items = ['Q42', 'Q5', 'Q1000', 'Q7'];
    utils.numericSortItems(items);
    assert.deepEqual(items, ['Q5', 'Q7', 'Q42', 'Q1000']);
});

test('sortRelations orders by prop then target', function () {
    const rels = [
        { from: 'Q1', prop: 'P26', to: 'Q3' },
        { from: 'Q1', prop: 'P22', to: 'Q5' },
        { from: 'Q1', prop: 'P22', to: 'Q2' }
    ];
    utils.sortRelations(rels);
    assert.equal(rels[0].prop, 'P22');
    assert.equal(rels[0].to, 'Q2');
    assert.equal(rels[1].prop, 'P22');
    assert.equal(rels[1].to, 'Q5');
    assert.equal(rels[2].prop, 'P26');
});
