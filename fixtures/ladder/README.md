# Ladder fixtures

Real-world oracle programs that gate the Symfony roadmap (plan track T4).
Each directory is a Composer project with `composer.json` + `composer.lock`
committed and `vendor/` + `var/` ignored; `tools/fixtures/setup.sh` installs
them with the stock php. `ladder.toml` in each fixture lists the rungs it
serves and the commands `cargo xtask ladder --rung <id>` runs under both `php`
and `rphp` for byte-comparison (ADR-034).

| Fixture | Rungs | Source |
|---|---|---|
| `L1-skeleton` | L1, L3, L6a, L7 | `composer create-project symfony/skeleton` (v8.1.99 → Symfony 8.1.7) |
