# Pre-edit salvage — T-SM-OAUTH-FIX-01

Origin: src/billing.rs + src/main.rs vóór OAuth-rate-limit-honor
Removed-in: commit-SHA wt/T-SM-BILLING-auto-detect post-fd3c712
Git-sha-pre: fd3c712
Reason: live Anthropic `/api/oauth/usage` endpoint geeft HTTP 429
  `retry-after: 3125s` na meerdere subscription-mode-toggle-tests.
  Backend cachte response niet en honoreerde retry-after niet → elke
  poll triggert opnieuw 429. Fix raakt 2 files (billing.rs error-detail
  + main.rs cache + deadline) — drievoudige redundantie.
Restore-cmd:
  cp salvage/T-SM-OAUTH-FIX-01/billing.rs.salvaged src/billing.rs
  cp salvage/T-SM-OAUTH-FIX-01/main.rs.salvaged src/main.rs
  cargo build --release
Verify-cmd: cargo test --workspace --no-fail-fast (baseline = 46/0)
