# Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io/), certificate
by [SignPath Foundation](https://signpath.org/).

## Roles

- Committer and reviewer: [Kyle Pelham](https://github.com/kylepelham)
- Approver: [Kyle Pelham](https://github.com/kylepelham)

Changes from other contributors require review before they are merged. The approver
reviews each signing request before releasing it.

## Process

Official Windows releases are built by GitHub Actions from versioned source in this
repository. Release tags must point to `master`, pass the repository's release policy and
CI checks, and be newer than every published version. SignPath verifies the build's origin
and signs approved artifacts produced by that workflow.

Tauri updater signatures are also generated during the release build. They authenticate
updates inside Drift and are separate from the Windows code-signing certificate.

See the [privacy policy](PRIVACY.md) for Drift's network and data behavior.
