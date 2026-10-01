// Resolve the owner from authenticated install credentials, never a client ID.
export function createCompanionEntitlementResolver({ db, verifyDownloadEntitlement }) {
  return async body => {
    const who = await verifyDownloadEntitlement(body);
    if (who.activationMode !== 'phaseAccount') return who;
    const license = db.prepare(`SELECT robloxUserId FROM licenses
      WHERE installId = ? AND token = ? ORDER BY updatedAt DESC LIMIT 1`)
      .get(who.installId, body.token);
    const owner = Number(license?.robloxUserId || 0);
    if (!Number.isSafeInteger(owner) || owner <= 0) return who;
    if (body.userId && Number(body.userId) !== owner) {
      const error = new Error('Wrong Roblox account.');
      error.status = 403;
      throw error;
    }
    return { ...who, userId: owner };
  };
}

export function installCompanionSessionAccess({ app, resolveEntitlement, hasEarlyAccess,
    isAccessBlocked, personalRelease }) {
  app.post('/plugin/build-access', async (req, res) => {
    res.set('Cache-Control', 'no-store');
    try {
      const who = await resolveEntitlement(req.body || {});
      const blocked = who.userId > 0 && isAccessBlocked(who.userId);
      const earlyAccess = !blocked && who.userId > 0 && await hasEarlyAccess(who.userId);
      res.json({ earlyAccess: Boolean(earlyAccess), status: blocked ? 'blocked' : 'verified',
        earlyAccessRelease: earlyAccess ? personalRelease(who.userId) : null });
    } catch (error) {
      res.status(error.status || 503).json({ earlyAccess: false, status: 'unavailable',
        earlyAccessRelease: null, message: error.status ? error.message : 'Access check is temporarily unavailable.' });
    }
  });
}
