# Verifying downloads

Starting with v0.1, each release on the [Releases page](https://github.com/Roboroads/repeat-that/releases) includes a
`SHA256SUMS` file with the SHA-256 hash of each installer. Compute the hash of the file you downloaded and compare it
with the line for the same file name in `SHA256SUMS`. The two must be identical. Letter case doesn't matter:
`Get-FileHash` prints uppercase hex and `SHA256SUMS` is lowercase.

Windows (PowerShell):

```powershell
Get-FileHash $HOME\Downloads\<file> -Algorithm SHA256
```

To let PowerShell do the comparison, paste the hash from `SHA256SUMS`. It prints `True` on a match, and `-eq` ignores
case:

```powershell
(Get-FileHash $HOME\Downloads\<file> -Algorithm SHA256).Hash -eq "<hash from SHA256SUMS>"
```

macOS:

```sh
shasum -a 256 ~/Downloads/<file>
```

Linux:

```sh
sha256sum ~/Downloads/<file>
```

On macOS and Linux the tool can also do the comparison. Save `SHA256SUMS` in the same folder as the download, then run
one of these there:

```sh
sha256sum -c SHA256SUMS --ignore-missing   # Linux
shasum -a 256 -c SHA256SUMS                # macOS
```

Your file must show `OK`. On macOS, `shasum` also reports the listed files you didn't download as `FAILED open or read`.
Ignore those lines.

`SHA256SUMS` sits on the same release page as the installers, so this catches a corrupted or incomplete download. It
doesn't prove who built the file.
