import { queueForItem } from '../write-queue.js';

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

test('tasks for the same item run in order', async () => {
    const seen = [];
    const first = queueForItem('Q1', async () => {
        await delay(10);
        seen.push('first');
        return 'a';
    });
    const second = queueForItem('Q1', async () => {
        seen.push('second');
        return 'b';
    });
    assert.deepEqual(await Promise.all([first, second]), ['a', 'b']);
    assert.deepEqual(seen, ['first', 'second']);
});

test('tasks for different items run concurrently', async () => {
    const order = [];
    const first = queueForItem('Q1', async () => {
        await delay(20);
        order.push('Q1');
    });
    const second = queueForItem('Q2', async () => {
        order.push('Q2');
    });
    await Promise.all([first, second]);
    assert.deepEqual(order, ['Q2', 'Q1'], 'the slow item does not hold up the other');
});

test('a queued task waits for the previous one to settle', async () => {
    let release;
    const gate = new Promise((resolve) => { release = resolve; });
    let second_started = false;
    const first = queueForItem('Q1', async () => { await gate; });
    const second = queueForItem('Q1', async () => { second_started = true; });
    await delay(10);
    assert.notOk(second_started, 'second task must not start before the first finishes');
    release();
    await Promise.all([first, second]);
    assert.ok(second_started);
});

test('a failing task does not block the next one', async () => {
    const failing = queueForItem('Q1', async () => { throw new Error('boom'); });
    const next = queueForItem('Q1', async () => 'ok');
    let threw = false;
    try {
        await failing;
    } catch (e) {
        threw = e.message === 'boom';
    }
    assert.ok(threw, 'the failure reaches its own caller');
    assert.equal(await next, 'ok');
});

test('an item can be queued again after its queue drained', async () => {
    await queueForItem('Q9', async () => 'first');
    assert.equal(await queueForItem('Q9', async () => 'second'), 'second');
});

// The claim path re-reads the live item before writing; queueing is what makes
// the second reader see the first writer's result instead of a duplicate.
test('the second task sees the first task\'s result', async () => {
    const server = new Set();
    const add = () => queueForItem('Q1', async () => {
        await delay(1);
        if (server.has('P22:Q2')) return 'existed';
        server.add('P22:Q2');
        return 'new';
    });
    const results = await Promise.all([add(), add()]);
    assert.deepEqual(results, ['new', 'existed']);
});
