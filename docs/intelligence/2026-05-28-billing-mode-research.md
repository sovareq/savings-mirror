# Billing-mode research — savings-mirror

Datum: 2026-05-28
Scope: distinguish Anthropic API pay-per-token billing from subscription billing (Pro/Max/Team/Enterprise) in a Rust dashboard tool.

---

## Q1. `/api/oauth/usage` endpoint

**Finding**
- URL: `https://api.anthropic.com/api/oauth/usage`
- Method: `GET` (no request body)
- Headers required:
  - `Authorization: Bearer <oauth_access_token>` (subscription OAuth access token, not an `sk-ant-` API key)
  - `anthropic-beta: oauth-2025-04-20`
  - `Content-Type: application/json` (cosmetic on GET)
- Response schema (JSON):
  ```json
  {
    "five_hour":  { "utilization": 0-100, "resets_at": "<ISO8601 UTC>" },
    "seven_day":  { "utilization": 0-100, "resets_at": "<ISO8601 UTC>" }
  }
  ```
- The endpoint is what Claude Code's own status-line uses to render the `5hr` and `wk` bars.
- It is undocumented on `platform.claude.com`; the only public, reproducible description is the community gist below, cross-checked against Claude Code's status-line behaviour.

**Source(s)**
- [Claude Code Status Line gist (Thomas L., 2026)](https://gist.github.com/thomaslty/72a86a5d539e8bca101ecc1528dc0948) — captured 2026-05-28
- Behaviour cross-checked against [Authentication - Claude Code Docs](https://code.claude.com/docs/en/authentication) — 2026-05-28

**Confidence**: medium — endpoint and schema unverified against an Anthropic-published spec; field names confirmed by community reverse-engineering only. If accuracy is critical, do a single live probe with a known subscription OAuth token and snapshot the response.

---

## Q2. OAuth credentials storage on macOS

**Finding**
- macOS: **Apple Keychain**, service name `Claude Code-credentials`, account = `$USER`.
- Stored value is a JSON blob; the relevant key is `claudeAiOauth` containing `accessToken`, `refreshToken`, `expiresAt`, `scopes`.
- There is **no** plain-text credentials file on macOS. (`~/.claude/.credentials.json` is the Linux/Windows fallback; on macOS it does not exist by default.)
- `~/.claude.json` exists and contains an `oauthAccount` metadata block (email, org name) but **no tokens**.
- Read example:
  ```bash
  security find-generic-password -s "Claude Code-credentials" -a "$USER" -w
  ```

**Source(s)**
- [Authentication - Claude Code Docs](https://code.claude.com/docs/en/authentication) — section "Credential management" — 2026-05-28
- [GitHub issue #9403 — Keychain persistence bug](https://github.com/anthropics/claude-code/issues/9403) — confirms service name
- [Recover OAuth token gist (shubcodes)](https://gist.github.com/shubcodes/3c9c7ff813715aa47018bf22e7cf8cb5) — 2026-05-28

**Confidence**: high

---

## Q3. `/status` output — Auth token field

**Finding**
The `/status` command renders a status panel with at minimum: `Version`, `Session name`, `Session ID`, `cwd`, `Auth token`, `Model`.

The `Auth token` field disambiguates the source:

| Active credential                                    | `Auth token` field shows                              |
|------------------------------------------------------|-------------------------------------------------------|
| `ANTHROPIC_API_KEY` set (sk-ant-…)                   | `ANTHROPIC_API_KEY`                                    |
| `ANTHROPIC_AUTH_TOKEN` set                           | `ANTHROPIC_AUTH_TOKEN`                                 |
| `CLAUDE_CODE_OAUTH_TOKEN` (long-lived OAuth env)     | `CLAUDE_CODE_OAUTH_TOKEN`                              |
| `apiKeyHelper` script                                | `apiKeyHelper`                                         |
| `/login` subscription (Pro/Max/Team/Enterprise)      | `Claude Account` (or similar — shows the logged-in account email/org, NOT a token literal) |

Practical detection rule for savings-mirror:
- If field literal starts with `ANTHROPIC_API_KEY` → pay-per-token.
- If field literal starts with `CLAUDE_CODE_OAUTH_TOKEN` or `ANTHROPIC_AUTH_TOKEN` → token-based but still possibly metered (gateway).
- If field shows an account/email or `Claude Account` style → subscription, then call `/api/oauth/usage` for caps.

**Source(s)**
- [Authentication - Claude Code Docs](https://code.claude.com/docs/en/authentication) — 2026-05-28
- [Claude Code Status Line gist](https://gist.github.com/thomaslty/72a86a5d539e8bca101ecc1528dc0948) — 2026-05-28

**Confidence**: medium — labels for `ANTHROPIC_*` and `CLAUDE_CODE_OAUTH_TOKEN` cases are verified; the **exact** literal shown for a pure `/login` subscription session is **unverified** from a primary source. Recommend snapshotting `/status` output once on a Max-subscribed shell before locking the parser.

---

## Q4. Env vars Claude Code reads for auth (precedence order)

**Finding** — official precedence per Anthropic docs (highest → lowest wins):

1. Cloud-provider toggles: `CLAUDE_CODE_USE_BEDROCK`, `CLAUDE_CODE_USE_VERTEX`, `CLAUDE_CODE_USE_FOUNDRY` (each enables AWS/GCP/Azure provider auth and supersedes the rest).
2. `ANTHROPIC_AUTH_TOKEN` — sent as `Authorization: Bearer …`. For LLM gateways/proxies.
3. `ANTHROPIC_API_KEY` — sent as `X-Api-Key`. Pay-per-token API key (`sk-ant-…`).
4. `apiKeyHelper` shell-script output (configured in `settings.json`, not an env var, but lives in precedence chain).
5. `CLAUDE_CODE_OAUTH_TOKEN` — long-lived OAuth token from `claude setup-token`. Counts against subscription quota.
6. `/login` subscription OAuth credentials (Keychain on macOS).

Related env vars worth knowing:
- `ANTHROPIC_BASE_URL` — route to custom endpoint.
- `CLAUDE_CONFIG_DIR` — relocates `.credentials.json` on Linux/Windows (no effect on macOS Keychain).
- `CLAUDE_CODE_API_KEY_HELPER_TTL_MS` — refresh interval for `apiKeyHelper`.

Note: `apiKeyHelper`, `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` apply to **terminal CLI** sessions only — Claude Desktop and remote sessions ignore them.

**Source(s)**
- [Authentication - Claude Code Docs](https://code.claude.com/docs/en/authentication) — "Authentication precedence" — 2026-05-28
- [Manage API key env vars - Help Center](https://support.claude.com/en/articles/12304248-manage-api-key-environment-variables-in-claude-code) — 2026-05-28

**Confidence**: high

---

## Q5. `reqwest` 0.12 + `rustls-tls` — POST JSON + bearer

**Finding**

`Cargo.toml`:
```toml
[dependencies]
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "http2"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

(`default-features = false` is required to fully avoid `openssl`; the default features pull in `native-tls`.)

Function pattern:
```rust
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct UsageReq {} // GET has no body; keep empty struct example for POST shape

#[derive(Deserialize, Debug)]
struct Window { utilization: u8, resets_at: String }

#[derive(Deserialize, Debug)]
pub struct UsageResp { pub five_hour: Window, pub seven_day: Window }

pub async fn fetch_usage(
    client: &reqwest::Client,
    base_url: &str,
    bearer: &str,
) -> reqwest::Result<UsageResp> {
    client
        .get(format!("{base_url}/api/oauth/usage"))
        .bearer_auth(bearer)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("content-type", "application/json")
        .send()
        .await?
        .error_for_status()?
        .json::<UsageResp>()
        .await
}
```

POST-with-JSON variant (if a future endpoint needs it):
```rust
client.post(url).bearer_auth(bearer).json(&body).send().await?
```

**Source(s)**
- [reqwest docs.rs](https://docs.rs/reqwest/) — 0.12.x stable, 2026-05-28
- [reqwest GitHub](https://github.com/seanmonstar/reqwest) — features list — 2026-05-28

**Confidence**: high

---

## Q6. `mockito` Rust — latest version + async test pattern

**Finding**
- Latest: **1.7.2** (docs.rs as of 2026-05-28; lib.rs / crates.io show 1.7.x family).
- **HTTP only** — no native HTTPS/TLS support. For HTTPS-bound code under test, inject the base URL (use `server.url()` as `base_url` in `fetch_usage`). Do **not** hard-code `https://api.anthropic.com`.

Pattern:
```rust
#[tokio::test]
async fn usage_parses_correctly() {
    let mut server = mockito::Server::new_async().await;

    let _m = server
        .mock("GET", "/api/oauth/usage")
        .match_header("authorization", "Bearer test-token")
        .match_header("anthropic-beta", "oauth-2025-04-20")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"five_hour":{"utilization":42,"resets_at":"2026-05-28T12:00:00Z"},
                       "seven_day":{"utilization":17,"resets_at":"2026-06-04T00:00:00Z"}}"#)
        .create_async()
        .await;

    let client = reqwest::Client::new();
    let resp = fetch_usage(&client, &server.url(), "test-token").await.unwrap();
    assert_eq!(resp.five_hour.utilization, 42);

    _m.assert_async().await;
}
```

**Source(s)**
- [mockito - crates.io](https://crates.io/crates/mockito) — 2026-05-28
- [mockito docs.rs](https://docs.rs/mockito/latest/mockito/) — 2026-05-28

**Confidence**: high

---

## Q7. Pro/Max/Team/Enterprise rate caps — May 2026

**Finding**
Anthropic **does not publish exact numeric 5-hour or 7-day caps** per tier. They are deliberately approximate and depend on prompt size, model, attached files, and current capacity.

Best public estimates (community + Anthropic's own "around N messages" language):

| Tier        | 5-hour (approx, light usage)          | 7-day (weekly) cap                     |
|-------------|---------------------------------------|----------------------------------------|
| Pro ($20)   | ~45 messages                          | Exists, unquantified                   |
| Max 5x      | ~225 messages (≈ 5× Pro)              | Exists, unquantified                   |
| Max 20x     | ~900 messages (≈ 20× Pro)             | Exists, unquantified                   |
| Team        | Per-seat, same shape as Pro/Max       | Per-seat weekly cap                    |
| Enterprise  | Per-seat (seat-based plans)           | Per-seat weekly cap                    |

May 2026 deltas:
- **2026-05-06**: Anthropic **doubled** the 5-hour Claude Code rate limits for Pro, Max, Team, seat-based Enterprise (SpaceX/Colossus 1 compute deal). Weekly limits **unchanged** at that point.
- **2026-05-13**: Anthropic raised the **weekly** Claude Code limits by **+50%** for all the same tiers, in effect **through 2026-07-13**.

Operational consequence for savings-mirror: do not hard-code numeric caps. Always read `five_hour.utilization` and `seven_day.utilization` from `/api/oauth/usage`. The endpoint reports a percentage, so caps need not be known.

**Source(s)**
- [Anthropic news — Higher limits + SpaceX (2026-05-06)](https://www.anthropic.com/news/higher-limits-spacex) — 2026-05-28
- [Appwrite blog — 5-hour limits doubled (2026-05-07)](https://appwrite.io/blog/post/anthropic-doubles-claude-code-rate-limits) — 2026-05-28
- [Pasquale Pillitteri — Weekly +50% through 2026-07-13](https://pasqualepillitteri.it/en/news/2494/claude-code-weekly-limits-50-percent-anti-codex-anthropic-2026) — 2026-05-28
- [TokenMix — Claude Limits 2026](https://tokenmix.ai/blog/complete-claude-limits-guide-2026-tokens-uploads-5-hour) — 2026-05-28

**Confidence**:
- Direction & timing of May 2026 changes: high.
- Numeric per-tier caps: **unverified** — Anthropic does not publish hard numbers; only "around N messages" estimates exist. Treat any per-tier number as approximate.

---

## Q8. Risk: calling `/api/oauth/usage` when subscription is capped

**Finding**
Two distinct rate-limit surfaces apply:

1. **Inference endpoints** (`/v1/messages` etc.) — when the subscription hits its 5h or 7d cap, the API returns `HTTP 429` with body `{"type":"error","error":{"type":"rate_limit_error","message":"..."}}` plus a `retry-after` header (seconds).
2. **`/api/oauth/usage`** itself — this is a metadata/quota-reporting endpoint. It is **expected** to remain reachable when a user is capped (otherwise the status-line could not render the bars at 100%). In observed behaviour it continues to return the full JSON with `utilization: 100` and a valid `resets_at`.

Other failure modes to handle defensively:
- **401 Unauthorized** — OAuth access token expired (they are short-lived; refresh via the `refreshToken` from Keychain). Handle by surfacing "re-login required" in the UI.
- **403 Forbidden** — token belongs to a deactivated/expired org seat, or token lacks the OAuth scope. Treat as "subscription invalid".
- **429** on the usage endpoint — possible if the dashboard polls aggressively. Respect `retry-after`; back off and cache the last good utilization snapshot.
- **5xx / network** — the usage endpoint is best-effort; do not block the UI. Show stale value with timestamp.
- **Empty / changed schema** — Anthropic can change this undocumented endpoint without notice. Wrap deserialization in a `Result<UsageResp, _>` and degrade gracefully to "unknown" rather than panicking.

API-key (`sk-ant-…`) callers calling `/api/oauth/usage` will most likely receive **401/403** because the endpoint requires the OAuth `anthropic-beta: oauth-2025-04-20` flow, not the API-key surface. savings-mirror should branch on auth-mode detection (Q3/Q4) and **not** call `/api/oauth/usage` for API-key sessions.

**Source(s)**
- [Anthropic Rate limits docs](https://platform.claude.com/docs/en/api/rate-limits) — 2026-05-28
- [Help Center — 429 errors](https://support.claude.com/en/articles/8114527-i-m-encountering-429-errors-and-i-m-worried-my-rate-limit-is-too-low-what-should-i-do) — 2026-05-28
- [GitHub issue #22876 — 429 despite dashboard quota](https://github.com/anthropics/claude-code/issues/22876) — confirms 429 shape — 2026-05-28

**Confidence**:
- Inference 429 shape: high.
- `/api/oauth/usage` behaviour when capped: **medium** — based on UX reasoning + status-line continuing to render; not confirmed against a published contract.
- 401/403 for API-key callers on this endpoint: medium — **unverified** by direct probe.

---

## Unverified summary (re-check before relying)
- `/api/oauth/usage` exact URL, schema, beta header value — only confirmed via one community gist.
- `/status` exact literal for a pure `/login` subscription session (vs env-var tokens).
- Numeric per-tier 5h/7d caps — Anthropic does not publish.
- `/api/oauth/usage` response when the user is at 100% cap, and the 401/403 behaviour when called with an `sk-ant-…` API key.

Recommend a one-time live probe in a controlled shell to snapshot:
1. `/status` output literals on (a) `ANTHROPIC_API_KEY` set, (b) `CLAUDE_CODE_OAUTH_TOKEN` set, (c) bare `/login` subscription.
2. `curl -i -H "Authorization: Bearer <oauth>" -H "anthropic-beta: oauth-2025-04-20" https://api.anthropic.com/api/oauth/usage`.

Lock the parser/contracts against those snapshots before shipping.
