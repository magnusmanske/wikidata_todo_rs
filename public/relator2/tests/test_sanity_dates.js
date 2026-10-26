if (typeof globalThis.Vue === 'undefined') {
    function VueStub() {}
    VueStub.observable = (o) => o;
    VueStub.set = (o, k, v) => { o[k] = v; };
    globalThis.Vue = VueStub;
}

const sanity = await import('../sanity.js');
const { timeValueToInterval, lifespansDisjoint, childBornAfterParentDeath, bornStrictlyAfter } = sanity;

const tv = (time, precision) => ({ time, precision });

test('timeValueToInterval brackets a year-precision value as a full year', function () {
    assert.deepEqual(timeValueToInterval(tv('+1850-00-00T00:00:00Z', 9)), { lo: 1850, hi: 1851 });
});

test('timeValueToInterval narrows for month and day precision', function () {
    const month = timeValueToInterval(tv('+1850-06-00T00:00:00Z', 10));
    assert.ok(month.lo > 1850 && month.hi < 1851, 'June 1850 sits inside the year');
    const day = timeValueToInterval(tv('+1850-06-15T00:00:00Z', 11));
    assert.ok(day.lo > month.lo - 0.001 && day.hi - day.lo < 0.01, 'day precision is a narrow slot');
});

test('timeValueToInterval rejects coarse precision and junk', function () {
    assert.equal(timeValueToInterval(tv('+1850-00-00T00:00:00Z', 8)), null); // decade
    assert.equal(timeValueToInterval(tv('+1850-00-00T00:00:00Z', 7)), null); // century
    assert.equal(timeValueToInterval(undefined), null);
    assert.equal(timeValueToInterval({ time: 'nonsense', precision: 11 }), null);
    assert.equal(timeValueToInterval({ time: '+1850-00-00T00:00:00Z' }), null);
});

test('timeValueToInterval handles BCE years', function () {
    assert.deepEqual(timeValueToInterval(tv('-0044-00-00T00:00:00Z', 9)), { lo: -44, hi: -43 });
});

test('lifespansDisjoint flags clearly separated lifespans only', function () {
    const b1700 = timeValueToInterval(tv('+1700-00-00T00:00:00Z', 9));
    const d1750 = timeValueToInterval(tv('+1750-00-00T00:00:00Z', 9));
    const b1800 = timeValueToInterval(tv('+1800-00-00T00:00:00Z', 9));
    const d1860 = timeValueToInterval(tv('+1860-00-00T00:00:00Z', 9));
    assert.equal(lifespansDisjoint(b1700, d1750, b1800, d1860), true);  // died 1750, born 1800
    assert.equal(lifespansDisjoint(b1800, d1860, b1700, d1750), true);  // order independent
    assert.equal(lifespansDisjoint(b1700, d1860, b1800, d1860), false); // overlapping lives
});

test('lifespansDisjoint stays quiet on the ambiguous same-year boundary', function () {
    const born1850 = timeValueToInterval(tv('+1850-00-00T00:00:00Z', 9));
    const died1850 = timeValueToInterval(tv('+1850-00-00T00:00:00Z', 9));
    // A died "1850" and B born "1850" might still overlap — don't warn.
    assert.equal(lifespansDisjoint(null, died1850, born1850, null), false);
});

test('lifespansDisjoint needs both a death and the other birth', function () {
    const born = timeValueToInterval(tv('+1800-00-00T00:00:00Z', 9));
    assert.equal(lifespansDisjoint(born, null, born, null), false);
});

test('childBornAfterParentDeath respects the father grace year', function () {
    const death1800 = timeValueToInterval(tv('+1800-00-00T00:00:00Z', 9));
    const born1802 = timeValueToInterval(tv('+1802-00-00T00:00:00Z', 9));
    const born1801 = timeValueToInterval(tv('+1801-00-00T00:00:00Z', 9));
    assert.equal(childBornAfterParentDeath(born1802, death1800, 1), true);  // >1y after father
    assert.equal(childBornAfterParentDeath(born1801, death1800, 1), false); // within grace
    assert.equal(childBornAfterParentDeath(born1801, death1800, 0), true);  // mother: any later year
});

test('bornStrictlyAfter compares non-overlapping birth windows', function () {
    const born1850 = timeValueToInterval(tv('+1850-00-00T00:00:00Z', 9));
    const born1800 = timeValueToInterval(tv('+1800-00-00T00:00:00Z', 9));
    assert.equal(bornStrictlyAfter(born1850, born1800), true);
    assert.equal(bornStrictlyAfter(born1800, born1850), false);
    assert.equal(bornStrictlyAfter(born1850, born1850), false);
    assert.equal(bornStrictlyAfter(null, born1800), false);
});
