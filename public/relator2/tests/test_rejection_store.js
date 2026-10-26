// Tests for the localStorage-backed suggestion rejection store. We stub Vue's
// reactive helpers and localStorage so the module can run under Node.
import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));

let storage = {};
globalThis.localStorage = {
    getItem(k)        { return Object.prototype.hasOwnProperty.call(storage, k) ? storage[k] : null; },
    setItem(k, v)     { storage[k] = String(v); },
    removeItem(k)     { delete storage[k]; },
    clear()           { storage = {}; },
};

function FakeVue() {}
FakeVue.observable = (o) => o;
FakeVue.set    = (o, k, v) => { o[k] = v; };
FakeVue.delete = (o, k)    => { delete o[k]; };
globalThis.Vue = FakeVue;

// Import after the stubs are in place.
const { appState } = await import(pathToFileURL(join(here, '..', 'app-state.js')).href);
const {
    suggestionKey,
    isRejected,
    rejectSuggestion,
    unrejectSuggestion,
    loadRejected,
} = await import(pathToFileURL(join(here, '..', 'rejection_store.js')).href);

const resetState = () => {
    storage = {};
    appState.rejected_suggestions = {};
};

test('suggestionKey is from|prop|to', () => {
    assert.equal(suggestionKey({ from: 'Q1', prop: 'P40', to: 'Q2' }), 'Q1|P40|Q2');
});

test('reject then check', () => {
    resetState();
    const s = { from: 'Q1', prop: 'P40', to: 'Q2' };
    assert.notOk(isRejected(s));
    rejectSuggestion(s);
    assert.ok(isRejected(s));
});

test('rejection persists across loadRejected', () => {
    resetState();
    rejectSuggestion({ from: 'Q1', prop: 'P40', to: 'Q2' });
    appState.rejected_suggestions = {};
    assert.notOk(isRejected({ from: 'Q1', prop: 'P40', to: 'Q2' }));
    loadRejected();
    assert.ok(isRejected({ from: 'Q1', prop: 'P40', to: 'Q2' }));
});

test('unreject removes the entry', () => {
    resetState();
    const s = { from: 'Q3', prop: 'P22', to: 'Q4' };
    rejectSuggestion(s);
    assert.ok(isRejected(s));
    unrejectSuggestion(s);
    assert.notOk(isRejected(s));
});

test('reject is idempotent', () => {
    resetState();
    const s = { from: 'Q5', prop: 'P25', to: 'Q6' };
    rejectSuggestion(s);
    rejectSuggestion(s);
    assert.equal(Object.keys(appState.rejected_suggestions).length, 1);
});

test('loadRejected tolerates corrupt storage', () => {
    resetState();
    storage['relator2:rejected_suggestions'] = '}{ this is not json';
    loadRejected();
    assert.equal(Object.keys(appState.rejected_suggestions).length, 0);
});
