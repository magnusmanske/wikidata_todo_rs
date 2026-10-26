// Tests for the login-state rules the tool relies on: a login check must only
// report "logged out" when the answer is unambiguous, and an edit failure must
// be recognised as an authentication problem so it can be retried/re-checked.
import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const { isAuthError, loginStateFromResponse } = await import(
    pathToFileURL(join(here, '..', '..', 'resources', 'vue_es6', 'widar.js')).href);

const rights = (userinfo) => ({ error: 'OK', result: { query: { userinfo } } });

test('logged in when the API names a user', () => {
    assert.equal(loginStateFromResponse(rights({ id: 42, name: 'Magnus' })), true);
});

test('anonymous userinfo counts as logged out', () => {
    assert.equal(loginStateFromResponse(rights({ id: 0, name: '1.2.3.4', anon: '' })), false);
    assert.equal(loginStateFromResponse(rights({ id: 0, name: '1.2.3.4' })), false);
});

test('an OAuth error is a definitive logged out', () => {
    assert.equal(loginStateFromResponse({ result: { error: { code: 'mwoauth-invalid-authorization' } } }), false);
});

test('other API errors say nothing about the login', () => {
    for (const code of ['readonly', 'maxlag', 'internal_api_error_DBQueryError']) {
        assert.equal(loginStateFromResponse({ result: { error: { code } } }), null, code);
    }
});

test('an answer without any login information is inconclusive', () => {
    for (const d of [undefined, {}, { error: 'OK', result: '' }, { result: { query: {} } }]) {
        assert.equal(loginStateFromResponse(d), null, JSON.stringify(d));
    }
});

test('authentication failures are told apart from other failures', () => {
    assert.ok(isAuthError('Not logged in. (How did that happen?)'));
    assert.ok(isAuthError("You haven't authorized this application yet! Go <a>here</a>"));
    assert.notOk(isAuthError('generic_action failed: bad claim'));
    assert.notOk(isAuthError(undefined));
});
