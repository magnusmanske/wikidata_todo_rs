import * as utils from '../utils.js';

test('nameHasSurname detects a surname already at the end', function () {
    assert.ok(utils.nameHasSurname('Johann von Decker', 'von Decker'));
});

test('nameHasSurname detects a surname anywhere in the name', function () {
    assert.ok(utils.nameHasSurname('von Decker Johann', 'von Decker'));
});

test('nameHasSurname is case-insensitive', function () {
    assert.ok(utils.nameHasSurname('johann VON decker', 'Von Decker'));
});

test('nameHasSurname ignores extra whitespace', function () {
    assert.ok(utils.nameHasSurname('Johann   von  Decker', 'von Decker'));
});

test('nameHasSurname does not match inside a longer word', function () {
    assert.notOk(utils.nameHasSurname('Johann Deckermann', 'Decker'));
});

test('nameHasSurname requires the full particle sequence', function () {
    assert.notOk(utils.nameHasSurname('Johann Decker', 'von Decker'));
});

test('nameHasSurname returns false when the surname is absent', function () {
    assert.notOk(utils.nameHasSurname('Johann Friedrich Wilhelm', 'von Decker'));
});

test('nameHasSurname returns false for empty/non-string input', function () {
    assert.notOk(utils.nameHasSurname('', 'Decker'));
    assert.notOk(utils.nameHasSurname('Johann Decker', ''));
    assert.notOk(utils.nameHasSurname(undefined, 'Decker'));
    assert.notOk(utils.nameHasSurname('Johann Decker', null));
});
