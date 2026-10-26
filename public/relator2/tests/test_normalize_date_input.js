const { normalizeDateInput, wikidataTimeValue } = await import('../utils.js');

test('free-text dates are parsed', function () {
    assert.equal(normalizeDateInput('2. Januar 1850'), '1850-01-02');
    assert.equal(normalizeDateInput('* 1850'), '1850');
    assert.equal(normalizeDateInput('† 3.4.1912'), '1912-04-03');
    assert.equal(normalizeDateInput('born around 1723 in Berlin'), '1723');
});

test('an already Wikidata-ready date is returned untouched', function () {
    // The point of the guard: parseDate would degrade these, because its
    // fallback only recognises a bare year.
    assert.equal(normalizeDateInput('1850-03'), '1850-03');
    assert.equal(normalizeDateInput('1850-3-2'), '1850-3-2');
    assert.equal(normalizeDateInput('1850-03-02'), '1850-03-02');
    assert.equal(normalizeDateInput('1850'), '1850');
});

test('every value the guard accepts really is Wikidata-ready', function () {
    for (const s of ['1850', '1850-03', '1850-3-2', '1850-03-02']) {
        assert.ok(wikidataTimeValue(normalizeDateInput(s)), s);
    }
});

test('unparseable and empty input yields an empty string', function () {
    assert.equal(normalizeDateInput('no date here'), '');
    assert.equal(normalizeDateInput(''), '');
    assert.equal(normalizeDateInput(undefined), '');
    assert.equal(normalizeDateInput(null), '');
    assert.equal(normalizeDateInput(1850), '');
});
