# Security Policy

## Supported versions

Security fixes are applied to the current development branch and the newest published
release. Older releases are not maintained as separate security branches.

| Channel | Supported |
| --- | --- |
| Current `master` | Yes |
| Latest release | Yes |
| Older releases | No |

## Report a vulnerability

Use GitHub's
[private vulnerability reporting](https://github.com/kylepelham/Drift/security/advisories/new)
to report a suspected vulnerability. Do not open a public issue, discussion, or pull
request before the report has been reviewed.

Include as much of the following as is safe to share:

- The affected Drift version or commit.
- The security impact and the boundary that was crossed.
- Minimal reproduction steps or a proof of concept.
- Relevant configuration with secrets removed.
- Any mitigation you have already identified.

Never include live provider keys, OAuth tokens, private prompts, proprietary source code,
or unredacted logs. The maintainer will investigate the report, coordinate a fix and
release when necessary, and credit reporters who want attribution.

## Scope

Useful reports include, but are not limited to:

- Exposure or bypass of the authenticated loopback sidecar.
- Bypass of permission, MCP approval, or exact-definition checks.
- Arbitrary code execution outside behavior explicitly authorized by the user.
- Leakage of credentials or sensitive workspace data across trust boundaries.
- Update signature verification or release-channel vulnerabilities.
- Unsafe path handling that escapes a documented workspace or configuration boundary.

Drift is a coding agent and intentionally runs approved tools with the current user's
permissions. A model editing files or running commands after authorization is not by
itself a vulnerability. Prompt injection and unsafe model output are relevant when they
bypass a Drift-enforced boundary.

If an issue exists entirely in the upstream OpenCode engine and is reproducible without
Drift, report it through [OpenCode's security policy](https://github.com/sst/opencode/security/policy).
You may still report it here when Drift changes the impact or exposes a separate boundary.
