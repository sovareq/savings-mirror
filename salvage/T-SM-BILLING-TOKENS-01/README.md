# Pre-edit salvage — T-SM-BILLING-TOKENS-01

Origin: src/caveman.rs + src/sovacount.rs + src/main.rs + assets/dashboard.html vóór tokens-primary-implementatie
Removed-in: commit-SHA wt/T-SM-BILLING-auto-detect post-94cfb0e
Git-sha-pre: 94cfb0e
Reason: backend en frontend tegelijk gewijzigd (cross-cut tokens-as-primary metric in subscription mode); drievoudige redundantie voor rollback
Restore-cmd:
  cp salvage/T-SM-BILLING-TOKENS-01/caveman.rs.salvaged src/caveman.rs
  cp salvage/T-SM-BILLING-TOKENS-01/sovacount.rs.salvaged src/sovacount.rs
  cp salvage/T-SM-BILLING-TOKENS-01/main.rs.salvaged src/main.rs
  cp salvage/T-SM-BILLING-TOKENS-01/dashboard.html.salvaged assets/dashboard.html
  cargo build --release
Verify-cmd: cargo test --workspace --no-fail-fast (baseline pre-tranche = 45/0; post-tranche = 46/0)
