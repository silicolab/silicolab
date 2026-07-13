# Security policy

## Supported versions

Security fixes are made on `main` and included in the next release. Only the
latest published release is supported; older releases do not receive separate
patches unless the maintainers announce otherwise.

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability. Email
[jiekangtian@gmail.com](mailto:jiekangtian@gmail.com) with a concise description
and enough information to reproduce or assess the issue.

Useful reports include:

- the affected version, commit, and operating system;
- the feature and trust boundary involved;
- reproduction steps or a minimal proof of concept;
- the expected and observed behavior;
- the potential impact and any known mitigations.

Do not send live API keys, SSH private keys, access tokens, proprietary project
files, or other credentials. Replace secrets with test values and state what
kind of credential was used.

The maintainers will acknowledge the report, validate its scope, and coordinate
a fix and disclosure with the reporter. Please allow time for a release to be
prepared before publishing details that would put users at risk.

## Security-sensitive areas

Reports are especially useful for issues involving:

- automatic update downloads and executable replacement;
- remote-worker download, checksum verification, SSH deployment, or execution;
- host-key verification and remote command construction;
- assistant API-key storage or unintended credential disclosure;
- project-file parsing, path traversal, or unsafe external-engine arguments.

Ordinary bugs and non-sensitive feature requests belong in the public issue
tracker.
