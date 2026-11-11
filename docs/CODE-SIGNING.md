# Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by
[SignPath Foundation](https://signpath.org).

Kestrel Download Manager's Windows programs (`Kestrel.exe`, `flux-nmhost.exe`, `flux-cli.exe`) and
its installer (`Kestrel-Setup-<version>-win-x64.exe`) are signed this way. They are built on GitHub's
servers from the public source code in this repository by
[`.github/workflows/release-signed.yml`](../.github/workflows/release-signed.yml); no file built on a
private computer is signed.

## Team roles

| Role | Members |
|---|---|
| Committers and reviewers | [Achraf Tsouli](https://github.com/AchrafTsouli) |
| Approvers | [Achraf Tsouli](https://github.com/AchrafTsouli) |

Every signing request is approved by hand by an approver.

## Privacy

Kestrel does not send any information to other networked systems unless the user asks for it:

- **Downloads** go only to the addresses the user or their browser gives it.
- **Update checks** read the release information from this project's GitHub releases. They can be
  turned off in Options → Updates.
- **Crash reports** are never sent automatically. When Kestrel has crashed, it offers to open a new
  GitHub issue with the report, which the user reads and sends themselves.

See also [PRIVACY.md](../PRIVACY.md).
