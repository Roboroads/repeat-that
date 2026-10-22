# Code signing policy

Windows builds of Repeat That aren't code-signed yet. The project is applying to the SignPath Foundation for free code
signing, and this page is the policy that applies from the day the application is approved. Until then Windows shows a
SmartScreen warning the first time you run the installer, and [install-windows.md](install-windows.md) explains how to
get past it. The macOS disk image is a separate case: it is signed with an Apple Developer ID certificate and notarized
by Apple, and this page doesn't cover it.

The source is public at [github.com/Roboroads/repeat-that](https://github.com/Roboroads/repeat-that), under the
[GNU General Public License v3.0 or later](../LICENSE).

## Attribution

Once the application is approved, this page will carry this line:

> Free code signing provided by [SignPath.io](https://signpath.io), certificate by
> [SignPath Foundation](https://signpath.org).

Until then no Repeat That file is signed with a SignPath certificate, and none claims to be.

## Team roles

Repeat That is a one-person project. Roboroads ([github.com/Roboroads](https://github.com/Roboroads)) is the only person
with write access to the repository, and fills every role:

- Committer and reviewer: Roboroads. Other people send pull requests, which Roboroads reviews and merges.
- Approver: Roboroads. Every signing request for a release is approved by hand before anything is signed.

## What gets signed

Two files, both for Windows: the installer, `Repeat-That_<version>_x64-setup.exe`, and the program inside it,
`repeat-that.exe`.

- They are built by GitHub Actions on GitHub-hosted runners, from the source at a release tag. Nothing that gets signed
  is built on a personal computer.
- Builds of pull requests are never signed.
- Only files built from this repository's source are signed. A file that another project made keeps its own signature,
  or none. The installer downloads Microsoft's WebView2 runtime when a PC doesn't have it, and that file is Microsoft's.
- The Linux packages and the macOS disk image aren't signed with this certificate.

The release's `SHA256SUMS` file stays the way to check that you have the file the build produced, whether it is signed
or not, see [verify-downloads.md](verify-downloads.md).

## Privacy

Repeat That doesn't send information to other networked systems unless you ask it to. Captured audio stays in memory
and is never saved or sent anywhere, and there is no account and no telemetry. The one thing the installer fetches is
Microsoft's WebView2 runtime, and only when it is missing. [PRIVACY.md](../PRIVACY.md) has the full text, including what
the website logs.

## Reporting a problem

If you think a signed file isn't what this page says it is, report it privately as described in
[SECURITY.md](../SECURITY.md).
