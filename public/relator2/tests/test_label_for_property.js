if (typeof globalThis.Vue === 'undefined') {
    function VueStub() {}
    VueStub.observable = (o) => o;
    VueStub.set = (o, k, v) => { o[k] = v; };
    globalThis.Vue = VueStub;
}

const { labelForProperty, property_label_overrides, named_properties } = await import('../relations.js');

const stubItem = (label, langs = {}) => ({
    getLabel(lang) {
        if (lang && langs[lang]) return langs[lang];
        return label;
    },
});

const stubWd = (map) => ({ getItem: (q) => map[q] });

test('student_of (P1066) is overridden to "teacher"', () => {
    assert.equal(property_label_overrides[named_properties.student_of], 'teacher');
    assert.equal(labelForProperty(named_properties.student_of, stubWd({})), 'teacher');
});

test('falls back to item label', () => {
    const wd = stubWd({ P22: stubItem('father') });
    assert.equal(labelForProperty('P22', wd), 'father');
});

test('respects language when supplied', () => {
    const wd = stubWd({ P22: stubItem('father', { de: 'Vater' }) });
    assert.equal(labelForProperty('P22', wd, 'de'), 'Vater');
    assert.equal(labelForProperty('P22', wd),       'father');
});

test('returns q when item not found', () => {
    assert.equal(labelForProperty('P9999', stubWd({})), 'P9999');
});

test('tolerates missing wd', () => {
    assert.equal(labelForProperty('P22', undefined), 'P22');
});
