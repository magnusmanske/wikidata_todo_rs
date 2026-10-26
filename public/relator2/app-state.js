import { WikiData } from '../resources/js_es6/wikidata.js';

const wd = new WikiData();

export const appState = Vue.observable({
    items: [],
    item_relations: {},
    main_q: '',
    source_url: '',
    rejected_suggestions: {},
    hovered_q: '',
});

export const bus = new Vue();

export function getWd() {
    return wd;
}

export function setMainQ(q) {
    appState.main_q = q;
}

export function setSourceUrl(url) {
    appState.source_url = url;
}

export function resetItems(newItems = []) {
    appState.items = [...newItems];
}

export function addItem(q) {
    if (!appState.items.includes(q)) appState.items.push(q);
}

export function setItemRelations(q, relations) {
    Vue.set(appState.item_relations, q, relations);
}

export function clearItemRelations() {
    appState.item_relations = {};
}
