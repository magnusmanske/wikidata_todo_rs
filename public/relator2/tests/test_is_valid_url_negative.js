import * as utils from '../utils.js';

test('isValidURL rejects javascript: scheme even with a domain-like tail', function () {
    assert.notOk(utils.isValidURL('javascript:alert(1)//.org'));
    assert.notOk(utils.isValidURL('JavaScript:alert(1)'));
});

test('isValidURL rejects data:, file:, vbscript:', function () {
    assert.notOk(utils.isValidURL('data:text/html,<script>alert(1)</script>'));
    assert.notOk(utils.isValidURL('file:///etc/passwd'));
    assert.notOk(utils.isValidURL('vbscript:msgbox(1)'));
});

test('isValidURL rejects ftp: and other non-http schemes', function () {
    assert.notOk(utils.isValidURL('ftp://example.org/x'));
    assert.notOk(utils.isValidURL('ssh://host/path'));
});

test('isValidURL rejects strings without a scheme', function () {
    assert.notOk(utils.isValidURL('example.com'));
    assert.notOk(utils.isValidURL('www.example.com/foo'));
});

test('isValidURL rejects garbage that just happens to contain a domain', function () {
    assert.notOk(utils.isValidURL('not a url example.com but contains one'));
});

test('isValidURL rejects non-string inputs', function () {
    assert.notOk(utils.isValidURL(null));
    assert.notOk(utils.isValidURL(undefined));
    assert.notOk(utils.isValidURL(42));
    assert.notOk(utils.isValidURL({}));
});

test('isValidURL still accepts canonical http(s) URLs with paths and queries', function () {
    assert.ok(utils.isValidURL('https://example.org/'));
    assert.ok(utils.isValidURL('http://wiki.example.com'));
    assert.ok(utils.isValidURL('https://example.org/foo?bar=baz#frag'));
});
