// Synchronous Wikidata-URL parsing for the drop-resolver. We re-implement the
// matcher here so the test stays pure (no need to load Vue / WikiData).
const WIKIDATA_URL = /^https?:\/\/(?:www\.)?wikidata\.org\/(?:wiki|entity)\/(Q\d+)\b/i;

function wikidataUrlToQid(url) {
    const m = url.match(WIKIDATA_URL);
    return m ? m[1].toUpperCase() : null;
}

test('wikidata URL with /wiki/ path', () => {
    assert.equal(wikidataUrlToQid('https://www.wikidata.org/wiki/Q12345'), 'Q12345');
});

test('wikidata URL with /entity/ path', () => {
    assert.equal(wikidataUrlToQid('https://www.wikidata.org/entity/Q42'), 'Q42');
});

test('wikidata URL without www subdomain', () => {
    assert.equal(wikidataUrlToQid('https://wikidata.org/wiki/Q7'), 'Q7');
});

test('wikidata URL with trailing fragment / query', () => {
    assert.equal(wikidataUrlToQid('https://www.wikidata.org/wiki/Q42#claims'), 'Q42');
    assert.equal(wikidataUrlToQid('https://www.wikidata.org/wiki/Q42?lang=de'), 'Q42');
});

test('wikidata lowercase qid is normalised', () => {
    assert.equal(wikidataUrlToQid('https://www.wikidata.org/wiki/q99'), 'Q99');
});

test('non-wikidata URL returns null', () => {
    assert.equal(wikidataUrlToQid('https://en.wikipedia.org/wiki/Berlin'), null);
    assert.equal(wikidataUrlToQid('https://www.wikidata.org/wiki/Property:P31'), null);
    assert.equal(wikidataUrlToQid('not a url'), null);
});
