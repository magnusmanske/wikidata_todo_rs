import * as utils from '../utils.js';

test('surnameFromLabel takes the last word for a plain name', function () {
    assert.equal(utils.surnameFromLabel('Heinrich Decker'), 'Decker');
});

test('surnameFromLabel keeps a "von" particle', function () {
    assert.equal(utils.surnameFromLabel('Heinrich von Decker'), 'von Decker');
});

test('surnameFromLabel keeps multi-word "de la" particles', function () {
    assert.equal(utils.surnameFromLabel('Juan de la Cruz'), 'de la Cruz');
});

test('surnameFromLabel keeps "van der" particles', function () {
    assert.equal(utils.surnameFromLabel('Anna van der Berg'), 'van der Berg');
});

test('surnameFromLabel handles many given names', function () {
    assert.equal(utils.surnameFromLabel('Johann Friedrich Wilhelm von Decker'), 'von Decker');
});

test('surnameFromLabel returns empty for a single word', function () {
    assert.equal(utils.surnameFromLabel('Madonna'), '');
});

test('surnameFromLabel returns empty for non-strings and blanks', function () {
    assert.equal(utils.surnameFromLabel(''), '');
    assert.equal(utils.surnameFromLabel('   '), '');
    assert.equal(utils.surnameFromLabel(undefined), '');
    assert.equal(utils.surnameFromLabel(null), '');
});

test('surnameFromLabel always leaves at least one given name', function () {
    // All-but-last are particles: keep the first word as the given name.
    assert.equal(utils.surnameFromLabel('von Decker'), 'Decker');
});

test('surnameFromLabel collapses extra whitespace', function () {
    assert.equal(utils.surnameFromLabel('  Heinrich   von   Decker '), 'von Decker');
});
