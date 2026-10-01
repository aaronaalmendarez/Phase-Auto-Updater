import assert from 'node:assert/strict';
import { createCompanionEntitlementResolver, installCompanionSessionAccess } from './companion-session-access.mjs';

let owner = 4945613323, credentialChecks = 0, ownerLookups = 0;
const db = { prepare: () => ({ get: (install, token) => {
  ownerLookups++;
  assert.equal(install, 'install'); assert.equal(token, 'valid');
  return { robloxUserId: owner };
} }) };
const resolve = createCompanionEntitlementResolver({ db, verifyDownloadEntitlement: async body => {
  credentialChecks++;
  if (body.token !== 'valid' || body.installId !== 'install') {
    const error = new Error('Invalid credentials'); error.status = 403; throw error;
  }
  return { activationMode: body.activationMode, userId: body.activationMode === 'phaseAccount' ? 0 : body.userId,
    installId: body.installId };
} });
const body = { activationMode: 'phaseAccount', installId: 'install', token: 'valid', userId: owner };
assert.equal((await resolve(body)).userId, owner);
assert.equal((await resolve({ ...body, userId: 0 })).userId, owner);
await assert.rejects(resolve({ ...body, userId: 123 }), { status: 403 });
const lookups = ownerLookups;
await assert.rejects(resolve({ ...body, token: 'bad' }), { status: 403 });
await assert.rejects(resolve({ ...body, installId: 'wrong' }), { status: 403 });
assert.equal(ownerLookups, lookups, 'Do not resolve identity before validating credentials');
owner = null;
assert.equal((await resolve({ ...body, userId: 0 })).userId, 0, 'Unbound licenses stay unbound');
owner = 4945613323;
assert.equal((await resolve({ ...body, activationMode: 'robloxPurchase' })).userId, owner);

let handler, eligible = true, blocked = false, releaseReads = 0;
installCompanionSessionAccess({ app: { post: (path, fn) => { assert.equal(path, '/plugin/build-access'); handler = fn; } },
  resolveEntitlement: resolve, hasEarlyAccess: async id => { assert.equal(id, owner); return eligible; },
  isAccessBlocked: id => { assert.equal(id, owner); return blocked; },
  personalRelease: id => { releaseReads++; assert.equal(id, owner); return { latestBuildId: 'ea11' }; } });
async function request(input) {
  const output = { code: 200, headers: {} };
  const res = { set: (k,v) => { output.headers[k] = v; return res; },
    status: code => { output.code = code; return res; }, json: value => { output.body = value; } };
  await handler({ body: input }, res);
  assert.equal(output.headers['Cache-Control'], 'no-store');
  return output;
}
let result = await request(body);
assert.equal(result.body.earlyAccess, true); assert.equal(result.body.status, 'verified');
assert.equal(result.body.earlyAccessRelease.latestBuildId, 'ea11');
eligible = false;
result = await request(body);
assert.equal(result.body.earlyAccess, false); assert.equal(result.body.earlyAccessRelease, null);
eligible = true; blocked = true;
result = await request(body);
assert.equal(result.body.status, 'blocked'); assert.equal(result.body.earlyAccess, false);
assert.equal(releaseReads, 1);
result = await request({ ...body, token: 'bad' });
assert.equal(result.code, 403); assert.equal(result.body.earlyAccess, false);
result = await request({ ...body, userId: 123 });
assert.equal(result.code, 403);
assert.equal(releaseReads, 1);
assert.ok(credentialChecks > 10);
console.log('PASS: authenticated identity, mismatch rejection, unbound accounts, role/revocation checks, no metadata on denial');
