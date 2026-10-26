#!/usr/bin/env node
import { readdirSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, join } from 'node:path';

const tests = [];

globalThis.test = (name, fn) => tests.push({ name, fn });

const fail = (msg, label) => { throw new Error(`${label || 'assert'}: ${msg}`); };
globalThis.assert = {
    equal(actual, expected, msg) {
        if (actual !== expected) fail(`expected ${JSON.stringify(expected)} but got ${JSON.stringify(actual)}`, msg || 'assert.equal');
    },
    deepEqual(actual, expected, msg) {
        const a = JSON.stringify(actual);
        const e = JSON.stringify(expected);
        if (a !== e) fail(`expected ${e} but got ${a}`, msg || 'assert.deepEqual');
    },
    ok(value, msg) {
        if (!value) fail(`expected truthy value, got ${JSON.stringify(value)}`, msg || 'assert.ok');
    },
    notOk(value, msg) {
        if (value)  fail(`expected falsy value, got ${JSON.stringify(value)}`,  msg || 'assert.notOk');
    },
};

const testDir = dirname(fileURLToPath(import.meta.url));
const files = readdirSync(testDir).filter((f) => /^test_.*\.js$/.test(f)).sort();

for (const f of files) await import(pathToFileURL(join(testDir, f)).href);

let passed = 0;
const failures = [];

for (const t of tests) {
    try {
        await t.fn();
        passed++;
        process.stdout.write('.');
    } catch (err) {
        failures.push({ name: t.name, err });
        process.stdout.write('F');
    }
}

process.stdout.write('\n');
if (failures.length) {
    console.log('\nFailures:');
    for (const { name, err } of failures) {
        console.log(`  - ${name}`);
        console.log(`      ${err.message}`);
    }
}
console.log(`\n${passed} passed, ${failures.length} failed (${tests.length} total)`);
process.exit(failures.length === 0 ? 0 : 1);
