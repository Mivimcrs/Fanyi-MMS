# Code Signing Policy — 莱·梵壹会员系统 (Fanyi-MMS)

Free code signing provided by [SignPath.io](https://signpath.io), certificate by
[SignPath Foundation](https://signpath.org).

## What gets signed

| Artifact | Signed by | Notes |
|---|---|---|
| `laifanyi-windows-x64.zip` (contains `laifanyi.exe`, 使用说明.txt) | SignPath Foundation certificate | exe is built exclusively by GitHub Actions from tagged releases of this repository |
| `laifanyi-macos-universal.zip` (`.app` bundle) | ad-hoc (unsigned / Gatekeeper notice) | disclosed intentionally; not covered by Foundation signing |

## Build and signing flow

1. A maintainer pushes a `vX.Y.Z` tag; GitHub Actions builds the binaries from that exact commit.
2. The Windows artifact is stored as a GitHub Actions workflow artifact, then submitted to
   SignPath.io via `submit-signing-request` (Trusted Build System: GitHub).
3. Every signing request requires **manual approval by the Approver** before SignPath signs it.
4. Signed artifacts are attached to the corresponding GitHub Release. Artifacts are never signed
   from developer workstations.

## Team roles

| Role | Member |
|---|---|
| Author | [@Mivimcrs](https://github.com/Mivimcrs) |
| Reviewer | [@Mivimcrs](https://github.com/Mivimcrs) |
| Approver | [@Mivimcrs](https://github.com/Mivimcrs) |

Sole-maintainer project; all three roles are held by the same person. Multi-factor
authentication is enabled on the GitHub account and on SignPath.io.

## Privacy statement

The application runs entirely locally: it listens on `127.0.0.1` only, stores all data in the
user's own Excel workbook, and performs **no network communication and no telemetry**.

## Contact

Issues: https://github.com/Mivimcrs/Fanyi-MMS/issues
