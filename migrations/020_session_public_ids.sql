-- Store sessions under the SHA-256 of their cookie token instead of the token.
--
-- Why: `sessions.id` was the `ctp_session` cookie value itself, and it was handed
-- out by GET /api/me/sessions, written to audit_events on every audited action and
-- placed in the DELETE /api/me/sessions/{id} URL (so proxy logs too). Anything that
-- could read one of those — an XSS, a browser extension, a log file or a backup —
-- got working cookies for every session of the user. The server now hashes the
-- cookie on each request (`auth::session::session_public_id`), so rewriting the
-- stored ids keeps every existing login valid.
UPDATE sessions SET id = encode(sha256(convert_to(id, 'UTF8')), 'hex');

UPDATE audit_events
SET actor_session_id = encode(sha256(convert_to(actor_session_id, 'UTF8')), 'hex')
WHERE actor_session_id IS NOT NULL;

UPDATE audit_events
SET resource_id = encode(sha256(convert_to(resource_id, 'UTF8')), 'hex')
WHERE resource_type = 'session' AND resource_id IS NOT NULL;
