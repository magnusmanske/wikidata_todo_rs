const MONTH_PATTERNS = [
    [/^jan/, 1],  [/^feb/, 2],  [/^m[aä]r/, 3], [/^apr/, 4],
    [/^ma[iy]/, 5], [/^jun/, 6], [/^jul/, 7],   [/^aug/, 8],
    [/^sep/, 9],  [/^o[ck]t/, 10], [/^nov/, 11], [/^de[cz]/, 12],
];

export function monthNameToNumber(monthStr) {
    const s = String(monthStr).toLowerCase().trim();
    for (const [pattern, month] of MONTH_PATTERNS) if (pattern.test(s)) return month;
    return undefined;
}

const DATE_ISO         = /(\d{4}-\d{2}-\d{2})/;
const DATE_DDMMYYYY    = /(\d{1,2})\. *(\d{1,2})\. *(\d{3,})/;
const DATE_DD_MONTH_YY = /(\d{1,2})\. *(\S+) (\d{3,})/;
const DATE_YEAR        = /(\d{3,})/;

const pad = (n) => String(n).padStart(2, '0');

export function parseDate(s) {
    if (typeof s !== 'string') return '';
    let m = s.match(DATE_ISO);
    if (m) return m[1];

    m = s.match(DATE_DDMMYYYY);
    if (m) {
        const day   = Number(m[1]);
        const month = Number(m[2]);
        const year  = m[3];
        if (day < 1 || day > 31 || month < 1 || month > 12) return year;
        return `${year}-${pad(month)}-${pad(day)}`;
    }

    m = s.match(DATE_DD_MONTH_YY);
    if (m) {
        const day   = Number(m[1]);
        const month = monthNameToNumber(m[2]);
        const year  = m[3];
        if (day < 1 || day > 31 || month === undefined) return year;
        return `${year}-${pad(month)}-${pad(day)}`;
    }

    m = s.match(DATE_YEAR);
    return m ? m[1] : '';
}

const START_TIME_PROP = 'P580';
const WIKIDATA_CALENDAR = 'http://www.wikidata.org/entity/Q1985727';

// Build a Wikidata `time` datavalue value from an ISO-ish string ("YYYY",
// "YYYY-MM" or "YYYY-MM-DD"), matching the format/precision logic of
// widar.newClaimDate so date qualifiers are consistent with the tool's other
// date claims. Returns undefined for anything that isn't a valid date.
export function wikidataTimeValue(s) {
    if (typeof s !== 'string') return undefined;
    let precision, year, month = '01', day = '01', m;
    if ((m = s.match(/^(\d+)-(\d+)-(\d+)$/))) {
        precision = 11; year = String(+m[1]); month = String(+m[2]); day = String(+m[3]);
    } else if ((m = s.match(/^(\d+)-(\d+)$/))) {
        precision = 10; year = String(+m[1]); month = String(+m[2]);
    } else if ((m = s.match(/^(\d+)$/))) {
        precision = 9; year = String(+m[1]);
    } else {
        return undefined;
    }
    return {
        time: `+${year}-${pad(month)}-${pad(day)}T00:00:00Z`,
        timezone: 0, before: 0, after: 0, precision, calendarmodel: WIKIDATA_CALENDAR,
    };
}

// Normalise a free-text date ("2. Januar 1850", "* 1850", "1850-03-02") to the
// ISO-ish form ("YYYY", "YYYY-MM", "YYYY-MM-DD") that wikidataTimeValue accepts.
// A value already in that form is returned untouched: parseDate's year-only
// fallback recognises neither "1850-03" nor "1850-3-2" and would silently drop
// the month and day. Returns '' when nothing date-like can be found.
export function normalizeDateInput(s) {
    if (!isNonEmptyString(s)) return '';
    return wikidataTimeValue(s) === undefined ? parseDate(s) : s;
}

// Build a "start time" (P580) qualifier map from an ISO-ish date string,
// suitable for a statement's `qualifiers`. Returns undefined when no valid
// date is given (so callers can simply skip adding a qualifier).
export function startTimeQualifier(s) {
    const value = wikidataTimeValue(s);
    if (!value) return undefined;
    return {
        [START_TIME_PROP]: [{
            snaktype: 'value',
            property: START_TIME_PROP,
            datavalue: { value, type: 'time' },
            datatype: 'time',
        }],
    };
}

export function toWikidataDate(date) {
    const d = date instanceof Date ? date : new Date(date);
    const year  = String(d.getUTCFullYear()).padStart(4, '0');
    const month = pad(d.getUTCMonth() + 1);
    const day   = pad(d.getUTCDate());
    return `+${year}-${month}-${day}T00:00:00Z`;
}

export function isValidURL(s) {
    if (typeof s !== 'string') return false;
    try {
        const u = new URL(s);
        return u.protocol === 'http:' || u.protocol === 'https:';
    } catch {
        return false;
    }
}

export function numericSortItems(items) {
    items.sort((a, b) => {
        const aNum = Number(a.slice(1));
        const bNum = Number(b.slice(1));
        return aNum - bNum;
    });
    return items;
}

export function isNonEmptyString(value) {
    return typeof value === 'string' && value !== '';
}

export function hasMirrorProperty(prop, mirrorPropertiesMap) {
    if (!mirrorPropertiesMap || typeof mirrorPropertiesMap !== 'object') return false;
    if (typeof prop !== 'string' || prop === '') return false;
    return Object.prototype.hasOwnProperty.call(mirrorPropertiesMap, prop);
}

// Of a set of pending statements, return the target QIDs that should be pulled
// into the view as family-tree cards. Only relation-property statements (P22,
// P26, …) introduce people; non-relation statements such as P21 (sex/gender) or
// drop-rule statements (P27 citizenship, P19 birthplace, …) point at items that
// are not relatives and must not be loaded as cards. Targets already in the view
// are skipped, and the result is de-duplicated while preserving order.
export function relationTargetsToLoad(statements, currentItems, relationProps) {
    const present   = new Set(currentItems);
    const relations = new Set(relationProps);
    const out = [];
    for (const s of statements) {
        if (!relations.has(s.prop)) continue;
        if (present.has(s.to) || out.includes(s.to)) continue;
        out.push(s.to);
    }
    return out;
}

// A description edit is worth saving only when it is a non-empty string that
// differs (ignoring surrounding whitespace) from the current value. This blocks
// both no-op saves and accidental removals (Wikidata trims descriptions, and we
// never want an empty value to delete an existing description here).
export function isSaveableDescription(draft, current) {
    if (typeof draft !== 'string') return false;
    const trimmed = draft.trim();
    return trimmed !== '' && trimmed !== (current ?? '').trim();
}

// Particles that commonly precede and belong to a surname ("von", "de la",
// "van der", …). Lower-cased; matching is case-insensitive.
const SURNAME_PARTICLES = new Set([
    'von', 'vom', 'zu', 'zur', 'van', 'der', 'den', 'ter', 'ten', 'te',
    'de', 'des', 'del', 'della', 'di', 'da', 'du', 'dos', 'das', 'do',
    'la', 'le', 'los', 'las', 'lo',
    'of', 'af', 'av', 'al', 'ibn', 'bin', 'y', 'e', 'i', 'st', 'st.',
]);

// Best-effort extraction of the family-name portion of a full personal name.
// Takes the last whitespace-separated word, then absorbs any immediately
// preceding nobiliary/linking particles ("von", "de la", "van der", …) so
// "Heinrich von Decker" yields "von Decker" and "Juan de la Cruz" yields
// "de la Cruz". Always leaves at least one leading word as the given name, and
// returns '' when the input has fewer than two words. This is a heuristic and
// cannot catch particle-less compound surnames (e.g. "García Márquez").
export function surnameFromLabel(label) {
    if (typeof label !== 'string') return '';
    const words = label.trim().split(/\s+/).filter(Boolean);
    if (words.length < 2) return '';
    let start = words.length - 1; // the last word is always part of the surname
    while (start > 1 && SURNAME_PARTICLES.has(words[start - 1].toLowerCase())) start--;
    return words.slice(start).join(' ');
}

// Whether `name` already contains `surname` as a whole-word sequence,
// case-insensitively (so appending it would be redundant). Whitespace is
// collapsed and the comparison is space-delimited, so "Decker" does not match
// inside "Deckermann" and "von Decker" matches only the full sequence.
export function nameHasSurname(name, surname) {
    if (!isNonEmptyString(name) || !isNonEmptyString(surname)) return false;
    const norm = (s) => ` ${s.toLowerCase().replace(/\s+/g, ' ').trim()} `;
    return norm(name).includes(norm(surname));
}

export function sortRelations(relations) {
    relations.sort((a, b) => {
        if (a.prop !== b.prop) return a.prop < b.prop ? -1 : 1;
        if (a.to   !== b.to)   return a.to   < b.to   ? -1 : 1;
        return 0;
    });
    return relations;
}
