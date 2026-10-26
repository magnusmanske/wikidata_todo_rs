import * as utils from '../utils.js';

const mirrors = {
    P22: ['P40'],
    P26: ['P26']
};

test('hasMirrorProperty returns true for known prop', function () {
    assert.equal(utils.hasMirrorProperty('P22', mirrors), true);
    assert.equal(utils.hasMirrorProperty('P26', mirrors), true);
});

test('hasMirrorProperty returns false for unknown prop', function () {
    assert.equal(utils.hasMirrorProperty('P999', mirrors), false);
});

test('hasMirrorProperty rejects falsy or malformed inputs', function () {
    assert.equal(utils.hasMirrorProperty('', mirrors), false);
    assert.equal(utils.hasMirrorProperty(undefined, mirrors), false);
    assert.equal(utils.hasMirrorProperty(null, mirrors), false);
    assert.equal(utils.hasMirrorProperty('P22', null), false);
});
