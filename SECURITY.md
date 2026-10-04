# Security Policy

## Supported versions

Griffin has no usable release yet. The crates on crates.io at version `0.0.0` only reserve names and contain no functionality. Once releases begin, this section will list which versions receive security fixes.

## Reporting a vulnerability

Please do not open a public issue for a security problem.

Report it privately through GitHub: go to the repository's **Security** tab and choose **Report a vulnerability**, or use <https://github.com/griffin-rs/griffin/security/advisories/new>.

Include what you found, how to reproduce it, and what an attacker could do with it. You can expect an acknowledgement within a week. We will agree a disclosure date with you and credit you in the advisory unless you prefer otherwise.

## Scope

In scope: the Griffin crates in this repository and the project that `cargo griffin new` generates, including its default session, CSRF, origin and header settings.

Out of scope: vulnerabilities in dependencies (report those upstream, though we want to know if Griffin's use of a dependency makes one exploitable) and in the Phoenix JavaScript client that Griffin vendors (report to the Phoenix project).
