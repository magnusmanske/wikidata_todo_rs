// Serialises writes per Wikidata item.
//
// Two edits to the same item must not race: the claim pre-check (fetch the live
// item, then add what is missing) would otherwise let both see the old state
// and add the same statement twice. Edits to different items still run fully in
// parallel, which is what makes background saving worthwhile.
//
// `task` is an async function; the returned promise settles with its result.
// A failing task does not block the tasks queued after it.

const tails = new Map();

export function queueForItem(q, task) {
    const previous = tails.get(q) ?? Promise.resolve();
    const result = previous.then(() => task());
    // The chain must stay usable after a failure; the caller still sees the
    // error through `result`.
    const tail = result.catch(() => {});
    tails.set(q, tail);
    tail.then(() => {
        // Only the last queued task clears the entry, so a task queued while
        // this one was running keeps the chain.
        if (tails.get(q) === tail) tails.delete(q);
    });
    return result;
}
