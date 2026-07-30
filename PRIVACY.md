# Privacy policy

Drift does not include maintainer-operated telemetry, analytics, advertising, or crash
reporting. The Drift maintainers do not receive your prompts, workspace contents,
credentials, or usage data.

## Local data

Drift stores application preferences, workspace metadata, and engine session data on your
computer. Provider credentials and OAuth tokens are managed locally by the bundled
OpenCode engine. Drift does not upload this local data to a Drift-operated service.

## Network connections

Drift connects to other systems only for features requested or configured by the person
installing or operating it:

- Installed copies check Drift's GitHub release endpoint for a signed update manifest on
  startup. Automatic checks can be disabled under **Settings > General**.
- Prompts, relevant context, tool results, and model settings are sent to the model
  provider selected by the user. That provider's privacy policy and terms apply.
- Provider authentication may connect to the provider's OAuth or API endpoints.
- MCP servers and plugins explicitly configured by the user may receive data or make
  network requests according to their own implementation and policies.

The bundled engine listens on an authenticated random port at `127.0.0.1`. This local
connection carries communication between Drift and its engine and is not a remote Drift
service.

## User control

Drift is a coding agent and can read files, modify code, and run commands with the current
user's permissions after authorization. Users control which workspaces, providers, MCP
servers, and plugins they configure. Removing Drift through Windows uninstalls the
application; provider-side data must be managed through the relevant provider.

Questions or suspected privacy issues can be reported through the repository's
[support and security channels](SECURITY.md).
