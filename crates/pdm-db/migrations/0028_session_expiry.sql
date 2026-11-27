-- Web sign-ins expire; API tokens do not.
--
-- `kind` tells a web sign-in (`web`, made only by POST /auth/login) from every other token
-- (`api`: tokens an administrator or `pdm-cli create-token` issues for CAD connectors, agents
-- and scripts). Only `web` tokens end, after an idle spell or at their lifetime
-- (PDM_SESSION_IDLE_MINUTES, PDM_SESSION_LIFETIME_MINUTES).
ALTER TABLE api_tokens ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'api';

-- Sign-ins made before this migration were all named `web-login` by /auth/login, with no
-- expiry. They become `web` so they end under the same rules (an old, unused one is refused on
-- its next use). An API token an administrator happened to name `web-login` stays `api`: its
-- creation is in the audit log (`user.token_created` with that name, for that user, recorded
-- within seconds of the token). A token made with `pdm-cli create-token --name web-login` has no
-- audit entry and is converted.
UPDATE api_tokens t SET kind = 'web'
 WHERE t.name = 'web-login' AND t.kind = 'api'
   AND NOT EXISTS (
       SELECT 1 FROM audit_events a
        WHERE a.action = 'user.token_created'
          AND a.org_id = t.org_id
          AND a.object_id = t.user_id
          AND a.detail ->> 'token_name' = 'web-login'
          AND a.at >= t.created_at
          AND a.at < t.created_at + interval '10 seconds');
