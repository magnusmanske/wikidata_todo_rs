import {
    drop_rules,
    findDropRule,
    dropRulePropertyIds,
    fallbackDropPropertyIds,
    GENERIC_DROP_PROPERTIES,
    DROP_RULE_INSTANCE_OF_PROP,
} from '../drop_rules.js';

function fakeItem(p31Values) {
    return {
        getClaimItemsForProperty(p) {
            if (p !== DROP_RULE_INSTANCE_OF_PROP) return [];
            return [...p31Values];
        },
    };
}

test('DROP_RULE_INSTANCE_OF_PROP is P31', function () {
    assert.equal(DROP_RULE_INSTANCE_OF_PROP, 'P31');
});

test('drop_rules ships with at least the documented place, cemetery, and educational_institution rules', function () {
    const names = drop_rules.map((r) => r.name);
    assert.ok(names.includes('place'), 'expected a "place" rule');
    assert.ok(names.includes('cemetery'), 'expected a "cemetery" rule');
    assert.ok(names.includes('educational_institution'), 'expected an "educational_institution" rule');
});

test('country rule maps Q6256/Q3624078 to country of citizenship (P27) only', function () {
    const c = drop_rules.find((r) => r.name === 'country');
    assert.deepEqual(c.properties, ['P27']);
    assert.ok(c.instance_of.includes('Q6256'));
    assert.ok(c.instance_of.includes('Q3624078'));
});

test('findDropRule routes Q6256 (country) to the country rule, not place', function () {
    const rule = findDropRule(fakeItem(['Q6256']));
    assert.ok(rule);
    assert.equal(rule.name, 'country');
});

test('findDropRule routes Q3624078 (sovereign state) to the country rule', function () {
    const rule = findDropRule(fakeItem(['Q3624078']));
    assert.ok(rule);
    assert.equal(rule.name, 'country');
});

test('an item tagged as both country and city still routes to country (rule order)', function () {
    const rule = findDropRule(fakeItem(['Q515', 'Q6256']));
    assert.equal(rule.name, 'country');
});

test('cemetery rule maps to place of burial (P119) only', function () {
    const cem = drop_rules.find((r) => r.name === 'cemetery');
    assert.deepEqual(cem.properties, ['P119']);
});

test('findDropRule matches Q39614 (cemetery) to the cemetery rule', function () {
    const rule = findDropRule(fakeItem(['Q39614']));
    assert.ok(rule);
    assert.equal(rule.name, 'cemetery');
    assert.equal(rule.properties.length, 1);
});

test('place rule offers P19, P20, P937', function () {
    const place = drop_rules.find((r) => r.name === 'place');
    assert.ok(place.properties.includes('P19'));
    assert.ok(place.properties.includes('P20'));
    assert.ok(place.properties.includes('P937'));
});

test('educational_institution rule offers P69 and P108', function () {
    const edu = drop_rules.find((r) => r.name === 'educational_institution');
    assert.ok(edu.properties.includes('P69'));
    assert.ok(edu.properties.includes('P108'));
});

test('findDropRule matches a city (Q515) to the place rule', function () {
    const rule = findDropRule(fakeItem(['Q515']));
    assert.ok(rule, 'expected a rule');
    assert.equal(rule.name, 'place');
});

test('findDropRule matches a university (Q3918) to the educational_institution rule', function () {
    const rule = findDropRule(fakeItem(['Q3918']));
    assert.ok(rule, 'expected a rule');
    assert.equal(rule.name, 'educational_institution');
});

test('findDropRule returns undefined for items with no matching instance_of', function () {
    assert.equal(findDropRule(fakeItem(['Q5'])), undefined); // Q5 = human
});

test('findDropRule returns undefined for items with no instance_of claims at all', function () {
    assert.equal(findDropRule(fakeItem([])), undefined);
});

test('findDropRule returns undefined for null / malformed input', function () {
    assert.equal(findDropRule(null), undefined);
    assert.equal(findDropRule(undefined), undefined);
    assert.equal(findDropRule({}), undefined);
});

test('findDropRule honours rule order: first match wins', function () {
    const customRules = [
        { name: 'first',  instance_of: ['Q123'], properties: ['P1'] },
        { name: 'second', instance_of: ['Q123'], properties: ['P2'] },
    ];
    assert.equal(findDropRule(fakeItem(['Q123']), customRules).name, 'first');
});

test('findDropRule tolerates malformed rules without crashing', function () {
    const customRules = [
        { name: 'bad-no-instances' },
        { name: 'bad-string-instances', instance_of: 'Q123' },
        { name: 'good', instance_of: ['Q42'], properties: ['P99'] },
    ];
    assert.equal(findDropRule(fakeItem(['Q42']), customRules).name, 'good');
});

test('dropRulePropertyIds returns the union of all properties across rules', function () {
    const customRules = [
        { name: 'a', instance_of: ['Q1'], properties: ['P1', 'P2'] },
        { name: 'b', instance_of: ['Q2'], properties: ['P2', 'P3'] },
    ];
    const ids = dropRulePropertyIds(customRules);
    assert.equal(ids.length, 3);
    assert.ok(ids.includes('P1'));
    assert.ok(ids.includes('P2'));
    assert.ok(ids.includes('P3'));
});

test('dropRulePropertyIds against the shipped rules includes P19 and P69', function () {
    const ids = dropRulePropertyIds();
    assert.ok(ids.includes('P19'));
    assert.ok(ids.includes('P69'));
});

test('dropRulePropertyIds does not include the generic-only properties', function () {
    assert.ok(!dropRulePropertyIds().includes('P463'));
});

test('fallbackDropPropertyIds adds generic properties (P463) to the rule union', function () {
    const ids = fallbackDropPropertyIds();
    assert.ok(ids.includes('P463'), 'expected member of (P463)');
    assert.ok(ids.includes('P19'), 'expected rule properties to still be present');
    // No duplicates.
    assert.equal(ids.length, new Set(ids).size);
    // Superset of both inputs.
    for (const p of GENERIC_DROP_PROPERTIES) assert.ok(ids.includes(p));
    for (const p of dropRulePropertyIds()) assert.ok(ids.includes(p));
});
