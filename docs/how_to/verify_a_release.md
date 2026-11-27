# Verify a release before installing it

Platform-Version: 0.1.0
Drill: scripts/verify/release.sh
Applies-To: any host, before the first install and before every upgrade

## Why

The artefact is verified **before** it is unpacked. A checksum checked afterwards has already
written whatever the artefact contained, and a file present but not listed in the manifest is
refused - a manifest that only checks what it names permits anything to be added to a release.

## Steps

0. On the build host, assemble the artefact and record it: `scripts/release/assemble.sh release
   <artefact-dir>`, which runs `bijective-deploy record <artefact-dir> <platform-version>`. The
   release workflow (`.github/workflows/release.yml`, run on demand) does the same on a clean
   runner, verifies the result and keeps the archive, its SHA-256 and the node image. The recorder and the verifier are
   the same code walking the same tree - a manifest written by a separate script agrees with
   that script's own assumptions.

1. Copy the release artefact to the host. **Nothing is fetched at install time** - no package
   index, no image registry, no download.
2. Run `bijective-deploy verify-artefact <artefact-dir>`.
3. A pass reports how many files were verified. **Any failure stops the install**; it is not a
   warning to note and continue past.

## If it refuses

| Message | What it means |
|---|---|
| `hashes to X, not Y` | The file differs from the release. Do not install; obtain the artefact again |
| `is listed and absent` | The artefact is incomplete |
| `is in the artefact and not in the manifest` | Something was added after the release was cut |

Do not delete the artefact. It is evidence.
