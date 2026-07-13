# Security Policy

## Reporting a vulnerability

Please report security issues privately to the repository owner instead of opening a public
issue. Include the affected version, reproduction steps, and impact, but remove every API key,
OAuth token, cookie, account identifier, and local file path that identifies a user.

Do not upload the AI Bucket AppData directory, provider authentication files, browser databases,
or raw HTTP request/response dumps. A sanitized response shape is usually sufficient.

## Credential handling

- API keys entered in AI Bucket are encrypted with Windows DPAPI for the current Windows user.
- Saved keys are masked in the interface and are never written to logs by design.
- Local Codex, Claude, and Antigravity credentials remain in their original app-owned locations.
- The project repository must not contain `.env` files, databases, credentials, tokens, signing
  keys, or generated release binaries.

The application uses unofficial structured quota endpoints for some providers. Treat a provider
endpoint change as a compatibility issue unless it exposes or transmits credentials outside the
expected provider domain.
