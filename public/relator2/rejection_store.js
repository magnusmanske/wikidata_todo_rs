// Persistent rejection list. Users can dismiss a suggestion they don't want;
// the (from, prop, to) tuple is remembered in localStorage so it doesn't come
// back on later page loads.
//
// The reactive copy lives on appState.rejected_suggestions so Vue templates
// can compute against it.

import { appState } from './app-state.js';

const STORAGE_KEY = 'relator2:rejected_suggestions';

export function suggestionKey(s) {
    return `${s.from}|${s.prop}|${s.to}`;
}

export function isRejected(s) {
    return Object.prototype.hasOwnProperty.call(appState.rejected_suggestions, suggestionKey(s));
}

export function loadRejected() {
    try {
        const raw = localStorage.getItem(STORAGE_KEY);
        if (!raw) return;
        const parsed = JSON.parse(raw);
        if (parsed && typeof parsed === 'object') {
            appState.rejected_suggestions = { ...parsed };
        }
    } catch {
        // Corrupted or unavailable storage — fall back to an empty set.
    }
}

function persist() {
    try {
        localStorage.setItem(STORAGE_KEY, JSON.stringify(appState.rejected_suggestions));
    } catch {
        // Quota / privacy mode — rejection still works for this session.
    }
}

export function rejectSuggestion(s) {
    const key = suggestionKey(s);
    if (Object.prototype.hasOwnProperty.call(appState.rejected_suggestions, key)) return;
    Vue.set(appState.rejected_suggestions, key, true);
    persist();
}

export function unrejectSuggestion(s) {
    const key = suggestionKey(s);
    if (!Object.prototype.hasOwnProperty.call(appState.rejected_suggestions, key)) return;
    Vue.delete(appState.rejected_suggestions, key);
    persist();
}
