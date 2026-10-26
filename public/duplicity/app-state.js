import { WikiData } from '../resources/js_es6/wikidata.js';

const wd = new WikiData();

export function getWd() {
    return wd;
}

export const bus = new Vue();
