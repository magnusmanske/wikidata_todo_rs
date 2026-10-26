// The login state must live in the widar module, not in the <widar> component:
// relator2 has a keyed <router-view>, so the component is rebuilt on every
// navigation. With the state on the instance, each navigation forgot that the
// user was logged in and showed the login prompt until a new check came back.
import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));

globalThis.location = { href: 'https://example.org/relator2/' };

const LOGGED_IN = { error: 'OK', result: { query: { userinfo: { id: 7, name: 'Magnus' } } } };
// Shaped like the real reply: GET api.php?action=get_rights while logged out.
const LOGGED_OUT = { error: 'OK', result: { error: {
    code: 'mwoauth-invalid-authorization',
    info: 'The authorization headers in your request are not valid: No approved grant was found for that authorization token.',
} } };

let calls = 0;
let answer = LOGGED_IN;
globalThis.fetch = async () => { calls++; return { ok: true, json: async () => answer }; };

const { ensureLoggedIn, isLoggedIn, isLoggedOut, loginState } = await import(
    pathToFileURL(join(here, '..', '..', 'resources', 'vue_es6', 'widar.js')).href);

test('a confirmed login is remembered, not re-fetched per caller', async () => {
    assert.equal(await ensureLoggedIn(), true);
    assert.ok(isLoggedIn(), 'logged in');
    assert.equal(loginState(), 'in');
    assert.equal(calls, 1);

    // What a route change (a new <widar> instance) does: it asks again.
    assert.equal(await ensureLoggedIn(), true);
    assert.equal(calls, 1, 'the fresh answer is reused, no second request');
});

test('concurrent callers share a single check', async () => {
    calls = 0;
    const results = await Promise.all([ensureLoggedIn(true), ensureLoggedIn(true), ensureLoggedIn(true)]);
    assert.deepEqual(results, [true, true, true]);
    assert.equal(calls, 1, 'one request for three callers');
});

test('a forced re-check picks up a login that went away', async () => {
    answer = LOGGED_OUT;
    assert.equal(await ensureLoggedIn(true), false);
    assert.ok(isLoggedOut(), 'logged out');
    assert.notOk(isLoggedIn());
});
