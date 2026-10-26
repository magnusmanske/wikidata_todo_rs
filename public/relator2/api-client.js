import { apiGetJSON } from '../resources/vue_es6/api-request.js';

export async function apiJson(url, params = {}) {
    try {
        return await apiGetJSON(url, params);
    } catch (err) {
        if (err instanceof Response) {
            const e = new Error(`HTTP ${err.status} ${err.statusText}`.trim());
            e.response = err;
            throw e;
        }
        throw err;
    }
}
