# Verifying downloads

Starting with v0.1, each release on the [Releases page](https://github.com/Roboroads/repeat-that/releases) includes a
`SHA256SUMS` file with the SHA-256 hash of each installer. Compute the hash of the file you downloaded and compare it
with the line for the same file name in `SHA256SUMS`. The two must be identical.

Windows (PowerShell):

```powershell
Get-FileHash .\<file> -Algorithm SHA256
```

macOS:

```sh
shasum -a 256 ~/Downloads/<file>
```

Linux:

```sh
sha256sum ~/Downloads/<file>
```

`SHA256SUMS` sits on the same release page as the installers, so this catches a corrupted or incomplete download. It
doesn't prove who built the file.
