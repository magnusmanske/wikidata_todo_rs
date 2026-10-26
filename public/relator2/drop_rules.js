// Centralised rule table for the "drop a Wikipedia link" feature.
//
// When the user drops a Wikipedia link on a person card, we resolve the link to
// a Wikidata item and inspect its `instance of` (P31) claims. The first rule
// whose `instance_of` set intersects those claims wins, and its `properties`
// are offered to the user as candidates for a transient statement on the
// dropped-on item.
//
// Edit this table by hand. Order matters: the first matching rule wins, so
// list more specific rules before broader ones.

export const DROP_RULE_INSTANCE_OF_PROP = 'P31';

// Properties always offered in the generic fallback list, shown when the dropped
// item's `instance of` doesn't match any specific rule above.
export const GENERIC_DROP_PROPERTIES = [
	'P463', // member of
];

export const drop_rules = [
	{
		name: 'country',
		instance_of: [
			'Q6256',    // country
			'Q3624078', // sovereign state
		],
		properties: [
			'P27', // country of citizenship
		],
	},
	{
		name: 'place',
		instance_of: [
			'Q515',     // city
			'Q3957',    // town
			'Q532',     // village
			'Q486972',  // human settlement
			'Q5119',    // capital
			'Q15284',   // municipality
			'Q1549591', // big city
			'Q253019',  // Ortsteil
			'Q484170',  // commune of France
			'Q42744322', // urban municipality in Germany
		],
		properties: [
			'P19',  // place of birth
			'P20',  // place of death
			'P937', // work location
		],
	},
	{
		name: 'profession',
		instance_of: [
			'Q28640',     // profression
			'Q66811410',  // medical profession
			'Q16335296',  // historical profession
		],
		properties: [
			'P106',     // occupation
		],
	},
	{
		name: 'cemetery',
		instance_of: [
			'Q39614',   // cemetery
			'Q108325',  // military cemetery
			'Q1242890', // city cemetery
			'Q284546',  // necropolis
		],
		properties: [
			'P119',     // place of burial
		],
	},
	{
		name: 'educational_institution',
		instance_of: [
			'Q3918',     // university
			'Q38723',    // higher education institution
			'Q875538',   // public university
			'Q902104',   // private university
			'Q1321960',  // research institute
			'Q9842',     // primary school
			'Q159334',   // secondary school
			'Q23002054', // private not-for-profit educational institution
			'Q3914',     // school
		],
		properties: [
			'P69',  // educated at
			'P108', // employer
		],
	},
];

export function findDropRule(item, rules = drop_rules) {
	if (!item || typeof item.getClaimItemsForProperty !== 'function') return undefined;
	const instances = item.getClaimItemsForProperty(DROP_RULE_INSTANCE_OF_PROP, true);
	if (!Array.isArray(instances) || instances.length === 0) return undefined;
	for (const rule of rules) {
		if (!Array.isArray(rule?.instance_of)) continue;
		if (rule.instance_of.some((q) => instances.includes(q))) return rule;
	}
	return undefined;
}

export function dropRulePropertyIds(rules = drop_rules) {
	const props = new Set();
	for (const rule of rules) {
		if (!Array.isArray(rule?.properties)) continue;
		for (const p of rule.properties) props.add(p);
	}
	return [...props];
}

// Property ids for the generic fallback list: the rule union plus the always-on
// generic properties offered when no specific rule matched.
export function fallbackDropPropertyIds(rules = drop_rules) {
	return [...new Set([...GENERIC_DROP_PROPERTIES, ...dropRulePropertyIds(rules)])];
}
