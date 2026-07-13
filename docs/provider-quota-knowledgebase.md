# AI Bucket Provider Quota Knowledgebase

Last reviewed: 2026-07-11

This document records quota and usage retrieval methods that are suitable for AI Bucket, a
standalone Windows viewer. It is based on source-code review of:

- [ouchanip/aimo](https://github.com/ouchanip/aimo), commit
  `68cc9a54e93132395133ab435727401059af8dcf`
- [diegosouzapw/OmniRoute](https://github.com/diegosouzapw/OmniRoute), commit
  `9cd18bf9a11b7d2e8c037c374631b492adabf469`

No user credential was sent to any provider while preparing this document. Endpoints marked
unofficial can change without notice and must fail safely.

## Product decision

AI Bucket supports only these credential strategies:

1. `local_credential`: cookie, access token, refresh token, or auth file already stored locally.
2. `oauth`: interactive OAuth/device login performed by AI Bucket, with tokens stored locally.
3. `api_key`: a provider API key entered or imported by the user.

HTML scraping is not a normal authentication strategy. It is an explicit last-resort transport
for a provider that exposes no usable JSON quota endpoint.

Preferred order:

1. Import an existing local credential when the official CLI/app already maintains it.
2. Offer OAuth when a public native/CLI flow is known and refreshable.
3. Use an API key when the quota endpoint accepts it.
4. Use cookie-authenticated JSON when no better option exists.
5. Parse HTML only when all JSON options are unavailable.

Do not read Chromium's encrypted cookie database directly. If a cookie is required, accept an
explicit import or use an app-owned login profile.

## Confidence levels

| Level | Meaning |
|---|---|
| A | Implemented in both projects, or implemented with a direct structured quota endpoint and clear response parsing. |
| B | Implemented in one project using an undocumented/internal endpoint. Requires fixtures and tolerant parsing. |
| C | Fallback, dashboard HTML parsing, inferred totals, or self-tracked data. Do not present as authoritative account quota. |

## Normalized data model

The old fixed `smallLimit` and `largeLimit` fields are insufficient. Codex, MiniMax, Claude and
other providers can return additional model-specific windows.

```ts
type AuthMethod = "local_credential" | "oauth" | "api_key";
type QuotaSource = "upstream_json" | "response_headers" | "html_fallback" | "local_estimate";

interface ProviderQuotaSnapshot {
  provider: string;
  accountId?: string;
  plan?: string;
  authMethod: AuthMethod;
  source: QuotaSource;
  fetchedAt: string;
  limits: QuotaWindow[];
  metadata?: Record<string, unknown>;
}

interface QuotaWindow {
  id: string;
  label: string;
  resource?: string;
  used?: number;
  total?: number;
  remaining?: number;
  usedPercent?: number;
  resetAt?: string;
  windowSeconds?: number;
  unlimited?: boolean;
  metadata?: Record<string, unknown>;
}
```

Persist each window as its own SQLite row, keyed by snapshot plus stable `id`. Preserve unknown
windows instead of discarding them.

## Target providers

| Provider | Preferred auth | Quota transport | Scrape required | Confidence |
|---|---|---|---|---|
| OpenAI Codex | Local `~/.codex/auth.json`, then OAuth | JSON `backend-api/wham/usage` | No | A/B |
| Google Gemini API | API key for API access only | No account-usage endpoint found in reviewed repos | No useful scrape identified | C |
| Google Antigravity | Google OAuth | Internal JSON `retrieveUserQuota` | No | B |
| MiniMax Coding/Token Plan | API key | JSON `token_plan/remains` or `coding_plan/remains` | No | A |
| GLM / Z.AI Coding Plan | API key or imported local JWT | JSON `usage/quota/limit` | No | A |

## OpenAI Codex

### Recommended path: local Codex auth file

- Credential file: `%USERPROFILE%\.codex\auth.json`
- Required fields:
  - `tokens.access_token`
  - `tokens.refresh_token` for recovery/refresh support
  - `tokens.id_token` for account and plan metadata
  - `tokens.account_id`, or derive the account from the ID-token claim
- Endpoint: `GET https://chatgpt.com/backend-api/wham/usage`
- Headers:
  - `Authorization: Bearer <access_token>`
  - `Accept: application/json`
  - `chatgpt-account-id: <account_id>` when known

The `chatgpt-account-id` header matters for users with Personal plus Team/Business workspaces.
Do not silently fall back to another workspace.

Expected response areas:

- `plan_type`
- `rate_limit.primary_window`
- `rate_limit.secondary_window`
- `rate_limit.limit_reached`
- `additional_rate_limits[]`
- optional `rate_limit_reset_credits`
- optional `rate_limit_reached_type`

For every rate-limit object, retain:

- `used_percent`
- `limit_window_seconds`
- `reset_at` or `reset_after_seconds`

`additional_rate_limits` can contain Spark/model-specific 5-hour and 7-day limits. This is the
main reason the AI Bucket model must use `limits[]`.

### OAuth path

OmniRoute implements two usable native-app patterns:

- Authorization Code with PKCE through `https://auth.openai.com/oauth/authorize`
- OpenAI's Codex device authorization flow through `https://auth.openai.com/codex/device`

The flow returns `access_token`, `refresh_token`, `id_token`, and expiry information. OAuth uses
the public Codex CLI client identifier; it relies on PKCE rather than client-secret secrecy.

Important refresh rule: OpenAI refresh tokens rotate. Serialize refresh operations and persist
the new access and refresh token atomically. Do not proactively refresh multiple accounts in
parallel, and do not refresh immediately after importing `auth.json`; a reused/stale refresh token
can invalidate the token family.

### Browser-cookie alternative

aimo's extension calls `GET https://chatgpt.com/api/auth/session` with ChatGPT cookies, extracts
`accessToken`, then calls `wham/usage`. This works but is unnecessary when Codex CLI auth or OAuth
is available. Keep it as an optional local-session import only, not as the default.

### Status

- Source: structured upstream JSON
- Scraping: not required
- Endpoint status: unofficial/internal
- Failure behavior: show `reauth_required` on 401/403; never expose token text

## Google Gemini API and Antigravity

These are separate products and must not share one quota collector.

### Gemini API key

An API key can list Gemini models and make inference calls, but neither aimo nor OmniRoute has a
working per-account consumed-quota endpoint for a normal Gemini API key. OmniRoute does not list
plain Gemini among its upstream usage-supported providers.

AI Bucket should therefore report:

- credential/configuration health;
- known static rate limits only when they are user-configured or sourced from official project
  metadata;
- no fabricated `usedPercent`.

Do not treat `GET /v1beta/models?key=...` as a quota endpoint. It validates access and returns
models, not consumed quota.

Future path: Google Cloud Monitoring/Service Usage may be possible with Google OAuth plus a GCP
project and IAM permissions, but that is a separate integration and was not demonstrated by the
reviewed repositories.

### Antigravity / AGY

- Auth: Google OAuth
- Token endpoint: `https://oauth2.googleapis.com/token`
- Quota endpoint:
  `POST https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota`
- Bootstrap/project endpoint family: `v1internal:loadCodeAssist`
- Response: per-model quota buckets and reset times

This is an internal Google Cloud Code API, not the Gemini API-key quota. Keep it as provider
`antigravity`, not `google-gemini`.

### Status

- Gemini API key consumed quota: unsupported/unknown
- Antigravity quota: structured JSON, no scrape, confidence B due to internal endpoints

## MiniMax

### Recommended path: API key

OmniRoute provides a non-scraping implementation for both international and China regions.

International endpoint order:

1. `GET https://www.minimax.io/v1/token_plan/remains`
2. `GET https://api.minimax.io/v1/api/openplatform/coding_plan/remains`

China endpoint order:

1. `GET https://www.minimaxi.com/v1/api/openplatform/coding_plan/remains`
2. `GET https://api.minimaxi.com/v1/api/openplatform/coding_plan/remains`

Headers:

- `Authorization: Bearer <api_key>`
- `Accept: application/json`

Relevant response fields:

- `current_subscribe_title`, `plan_name`, or related plan fields
- `model_remains[]`
- `model_name`
- `current_interval_total_count`
- `current_interval_usage_count`
- `current_interval_remaining_percent`
- `remains_time` or `end_time`
- weekly equivalents: `current_weekly_*`, `weekly_remains_time`, `weekly_end_time`

Coding Plan responses may contain percentage-only windows with zero counts. Token Plan responses
may contain absolute counts. Support both. For `/coding_plan/remains`, count fields can represent
remaining rather than used; derive `used = total - remaining` defensively.

Filter to text/coding model groups such as `general`, `minimax-m*`, or `coding-plan*`; do not merge
video/image/music pools into the coding quota.

### Cookie fallback from aimo

aimo calls:

`GET https://platform.minimax.io/v1/api/openplatform/coding_plan/remains?GroupId=<group_id>`

It uses the browser session cookie and reads `minimax_group_id_v2`. This path is useful only when
the user's subscription credential is not accepted as a bearer API key. It is cookie-authenticated
JSON, not HTML scraping.

### Status

- Preferred source: API-key authenticated JSON
- Fallback: explicit local cookie/session import
- HTML scraping: not required
- Confidence: A

## GLM / Z.AI

### Recommended path: API key

International:

`GET https://api.z.ai/api/monitor/usage/quota/limit`

China:

`GET https://open.bigmodel.cn/api/monitor/usage/quota/limit`

Headers:

- `Authorization: Bearer <api_key_or_jwt>`
- `Accept: application/json`

Team coding plans may require:

- query `type=2`
- `bigmodel-organization: <organization_id>`
- `bigmodel-project: <project_id>`

Relevant response fields under `data.limits[]`:

- `type`: `TOKENS_LIMIT` or `TIME_LIMIT`
- `percentage`
- `usage`, `remaining`, `currentValue`
- `nextResetTime`
- `unit`, `number`
- optional per-model `models[]`
- optional `usageDetails[]`

Map time windows from `unit` and `number` rather than assuming every account has exactly 5-hour
and weekly limits. Preserve monthly tool/MCP quotas separately.

### Local JWT alternative

aimo imports/captures a Z.AI JWT from local browser storage and sends it as the same Bearer token.
AI Bucket may support explicit token import, but should not inject scripts into another browser.

### Status

- Source: structured upstream JSON
- Auth: API key or imported local token
- HTML scraping: not required
- Confidence: A

## Additional reusable providers

These are not in the original AI Bucket scope, but both repositories reveal collectors that fit
the three approved auth strategies.

| Provider | Auth | Upstream quota/usage endpoint | Notes | Confidence |
|---|---|---|---|---|
| Claude Code | OAuth | `GET https://api.anthropic.com/api/oauth/usage` | Returns 5h, 7d and model-specific utilization; legacy admin fallback uses `/v1/settings` plus organization usage. | A |
| GitHub Copilot | OAuth token | `GET https://api.github.com/copilot_internal/user` | Premium interactions, chat and completions quota. Internal endpoint. | B |
| Kimi Coding | OAuth or API key | `GET https://api.kimi.com/coding/v1/usages` | API key uses `x-api-key`; OAuth uses Bearer plus device headers. | A |
| Cursor | Imported local access token | `POST https://cursor.com/api/dashboard/get-current-period-usage` | Token can be imported from Cursor's local `state.vscdb`; internal dashboard endpoint. | B |
| Kiro / Amazon Q | OAuth | AWS CodeWhisperer/Q `GetUsageLimits` endpoint | Requires OAuth profile/region metadata; structured response. | B |
| CodeBuddy CN | OAuth/device token | `POST https://copilot.tencent.com/v2/billing/meter/get-user-resource` | Structured billing resource response. | B |
| Qoder | PAT/API key | Exchange PAT for job token, then `GET https://openapi.qoder.sh/api/v3/user/status` | Plan and quota/exhaustion status. | B |
| DeepSeek | API key | `GET https://api.deepseek.com/user/balance` | Returns currency balances, not rolling request windows. | A |
| NanoGPT | API key | `GET https://nano-gpt.com/api/subscription/v1/usage` | Daily/weekly token and image quotas. | B |
| CrofAI | API key | `GET https://crof.ai/usage_api/` | Returns remaining requests and credits, but no authoritative total/reset. | B/C |
| Alibaba Bailian Coding Plan | API key/console key | Console coding-plan quota endpoint | Returns 5h, weekly and monthly windows; endpoint is console/internal. | B |
| OpenCode / OpenCode Zen | API key | `GET https://opencode.ai/zen/go/v1/quota` | Endpoint may return 404 and is not considered stable/public. | C |

## Providers that still need HTML fallback

### OpenCode Go dashboard

- Cookie: `auth`
- Workspace ID required
- Page: `https://opencode.ai/workspace/<workspace_id>/go`
- Extracts rolling, weekly and monthly usage from server-rendered HTML

The attempted API-key route through Z.AI quota can reject valid OpenCode keys. OmniRoute explicitly
falls back to workspace ID plus auth cookie. Keep this integration disabled by default and label it
`html_fallback`.

### Ollama Cloud

- Cookie: `__Secure-session`
- Page: `https://ollama.com/settings`
- Extracts session and weekly progress tracks from HTML

No structured quota endpoint was found in either reviewed repository. This is a valid exception to
the no-scrape policy, but parsing must be isolated and covered by captured HTML fixtures.

## Cookie-authenticated JSON that is not scraping

Cookie use and HTML scraping are different concerns. These aimo paths return JSON and may remain
acceptable as local-credential fallbacks:

| Provider | Endpoint | Credential |
|---|---|---|
| Codex | `/api/auth/session` then `/backend-api/wham/usage` | ChatGPT session cookie |
| Claude | `/api/bootstrap` then `/api/organizations/<id>/usage` | Claude session cookie |
| MiniMax | `/v1/api/openplatform/coding_plan/remains?GroupId=...` | MiniMax session cookie plus GroupId cookie |
| Z.AI | `/api/monitor/usage/quota/limit` | Imported JWT from local storage |

Prefer OAuth/API-key alternatives where available because cookie expiry and manual import are harder

### Claude Desktop on Windows

- Discover the Microsoft Store package dynamically from `%LOCALAPPDATA%\\Packages\\Claude_*`;
  do not hardcode its version or publisher suffix.
- Electron profile: `LocalCache\\Roaming\\Claude`.
- Chromium encryption metadata: `Local State` (`os_crypt.encrypted_key`, protected by Windows DPAPI).
- Session database: `Network\\Cookies`; the required `sessionKey` is Chromium `v10` AES-GCM data.
- Decrypt credentials only in memory. Do not persist cookie values or include them in logs/errors.
- Fetch `GET https://claude.ai/api/bootstrap`, select the active subscription organization, then call
  `GET https://claude.ai/api/organizations/{org_id}/usage` with the local session cookie.
- The cookie database can be locked while Claude Desktop is running. Surface a quit-and-refresh
  instruction instead of modifying or forcibly unlocking the profile.
to support safely.

## Methods excluded from AI Bucket

OmniRoute also reports some quota-like data by counting only requests routed through OmniRoute.
That is appropriate for a gateway, but not authoritative for a standalone viewer.

Exclude these methods:

- Vertex AI spend calculated from OmniRoute's local usage history
- xAI/Grok token usage calculated from OmniRoute's local history
- Xiaomi MiMo monthly usage calculated from OmniRoute's local history
- Qwen's "tracked per request" placeholder
- any static free-tier calculation presented as actual consumed account quota

AI Bucket can display local estimates only if they are explicitly labeled `local_estimate`; they
must never replace upstream account usage.

## Credential storage and security

- Never store provider tokens in frontend `localStorage`.
- Keep secrets in the Rust backend and encrypt them with Windows DPAPI or Windows Credential
  Manager. SQLite should store only encrypted blobs or secret references.
- Imported auth files should be parsed in memory. Store only fields needed for refresh and quota.
- Never log cookies, API keys, authorization headers, raw JWTs, or raw provider responses that may
  contain identity data.
- Mask account identifiers in diagnostics.
- Set strict allowlisted hosts per provider; do not let credentials follow arbitrary redirects.
- Use request timeouts and response-size limits.
- Serialize rotating OAuth refreshes per provider/account and persist token rotation atomically.
- A 401/403 should move the account to `reauth_required`, not erase history.

## Refresh policy

- Manual refresh is always allowed.
- Default background refresh: 10 minutes for stable JSON endpoints.
- Minimum provider refresh interval: 5 minutes unless the provider documents otherwise.
- Cache repeated refresh clicks for 30-60 seconds.
- Use exponential backoff for 429/5xx/network errors.
- Do not retry 401/403 until credential refresh or user re-authentication succeeds.
- Store the last successful snapshot separately from the latest error so temporary failures do not
  erase visible quota data.

## Recommended implementation order

1. Change frontend, Rust and SQLite models to `limits[]`.
2. Implement Codex import from `~/.codex/auth.json` and `wham/usage`.
3. Add Codex OAuth/device login as a fallback and token-refresh path.
4. Add Claude OAuth and `api.anthropic.com/api/oauth/usage` to the first delivery phase.
5. Implement MiniMax API-key collector.
6. Implement GLM/Z.AI API-key collector.
7. Keep Gemini API-key provider in configuration-health mode until an authoritative consumed-quota
   source is verified.
8. Add Antigravity OAuth as a separate Google provider.
9. Defer OpenCode, Ollama Cloud and GitHub Copilot to a later phase.
10. Add HTML fallbacks only behind an explicit per-provider setting.

## Delivery phases

### Phase 1

- OpenAI Codex: local `auth.json`, OAuth fallback, structured usage JSON
- Claude Code: OAuth, structured usage JSON
- MiniMax: API key, cookie-authenticated JSON fallback only if required
- GLM / Z.AI: API key or imported local JWT
- Google Antigravity: local desktop OAuth collector using `state.vscdb`, `loadCodeAssist`, and
  `retrieveUserQuota`; this replaces the Gemini API placeholder in AI Bucket

### Later phase

- OpenCode / OpenCode Go
- Ollama Cloud
- GitHub Copilot

OpenCode and Ollama remain later-phase because their reliable viewer paths currently depend on
HTML parsing. Copilot has a structured endpoint but is outside the first delivery set.

## Source map

aimo:

- `collectors/codex.mjs`
- `collectors/zai.mjs`
- `collectors/ollama.mjs`
- `extension/fetchers.js`

OmniRoute:

- `open-sse/services/usage/codex.ts`
- `open-sse/services/codexUsageQuotas.ts`
- `open-sse/services/codexQuotaFetcher.ts`
- `open-sse/services/usage/minimax.ts`
- `open-sse/services/usage/glm.ts`
- `open-sse/config/glmProvider.ts`
- `open-sse/services/usage/claude.ts`
- `open-sse/services/usage/kimi.ts`
- `open-sse/services/usage/antigravity.ts`
- `open-sse/services/opencodeOllamaUsage.ts`
- `open-sse/services/opencodeQuotaFetcher.ts`
- `open-sse/services/deepseekQuotaFetcher.ts`
- `src/lib/oauth/constants/oauth.ts`
- `src/lib/oauth/utils/codexAuthImport.ts`

Both repositories are MIT-licensed. Attribution is recorded in
[`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md).
