# setup-unemu

A composite GitHub Action that installs `unemu`, the desktop app and command line,
from a release of [unemu-org/unemu](https://github.com/unemu-org/unemu),
so another repository's workflow can import images and media sets and push
them with the same CLI and the same publication guard.

The action downloads `unemu-<version>-<target>.tar.gz` from the release
with curl (HTTPS only, with retries, no token), checks it against the
release's own `SHA256SUMS` before unpacking anything (a check of the download,
not a pin), and unpacks only `unemu`, `LICENSE`
and `THIRD-PARTY-LICENSES.md` into a new directory under `$RUNNER_TEMP`. It
adds that directory to `PATH` and prints `unemu --version`. It uses no other
actions and no secrets.

Runners: Linux X64 (`x86_64-unknown-linux-gnu`, such as `ubuntu-22.04`),
Linux ARM64 (`aarch64-unknown-linux-gnu`, such as `ubuntu-22.04-arm`),
macOS ARM64 (`aarch64-apple-darwin`, such as `macos-latest`) and
macOS X64 (`x86_64-apple-darwin`, such as `macos-15-intel`). Both Linux
architectures require glibc 2.35 or newer; install `libasound2` or the
distribution's ALSA runtime equivalent before this action, even on headless
runners. Any other runner fails with a message naming its platform.

## Usage

Pin the action to a full commit of unemu and name the release:

```yaml
- uses: unemu-org/unemu/.github/actions/setup-unemu@<unemu commit SHA>
  with:
    version: v0.0.4
```

The commit pins the action's own code; `version` names the release whose
binary it installs, for the runner's platform.

For pre-split PC releases through v0.0.3, use `setup-upcli` at an older
`unemupc` commit. Those releases belong to the PC core repository.

## Inputs

| Input | Required | Description |
| --- | --- | --- |
| `version` | yes | Release tag, such as `v0.0.4`; it must match `^v[0-9][0-9A-Za-z.+-]*$`. |
| `commit` | no | Full commit (40 lowercase hex digits) the release was built from. When set, the `commit:` line of `unemu --version` must name it, and the step fails on another commit or on no commit line. Use a release that ships the combined `unemu` binary (v0.0.4 or newer). |

A tag can be moved, a commit cannot: `commit` checks that the installed
binary was built from the source you expect. Find a
release's commit with `gh api repos/unemu-org/unemu/commits/<tag> --jq .sha`.

## Outputs

| Output | Description |
| --- | --- |
| `path` | Absolute path of the installed `unemu` binary. |

## Example: import a media set and push it

```yaml
name: Publish media
on:
  workflow_dispatch:

permissions:
  contents: read

jobs:
  publish:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@<commit SHA> # pin by commit
        with:
          persist-credentials: false

      - uses: unemu-org/unemu/.github/actions/setup-unemu@<unemu commit SHA>
        with:
          version: v0.0.4

      - name: Import the media set
        run: |
          set -euo pipefail
          unemu image media import media/disk1.img media/disk2.img \
            --title "Example installation set" --tag unemu/media/example:1.0.0 \
            --extra media/LICENSE.TXT:LICENSE.TXT --extra media/SOURCE.TAR:SOURCE.TAR \
            --open --license GPL-2.0-or-later --license-text LICENSE.TXT --source SOURCE.TAR \
            --store "$RUNNER_TEMP/store" --create

      # The guard's verdict and every blob's class, without network access or
      # credentials.
      - name: Check the publication guard
        run: unemu image push unemu/media/example:1.0.0 --store "$RUNNER_TEMP/store" --dry-run

      # Only this step sees the registry credentials.
      - name: Push the media set
        env:
          UNEMU_REGISTRY_HOST: registry.unemu.dev
          UNEMU_REGISTRY_USER: ${{ secrets.UNEMU_REGISTRY_USER }}
          UNEMU_REGISTRY_PASSWORD: ${{ secrets.UNEMU_REGISTRY_PASSWORD }}
        run: unemu image push unemu/media/example:1.0.0 --store "$RUNNER_TEMP/store"
```

`UNEMU_REGISTRY_USER` and `UNEMU_REGISTRY_PASSWORD` are sent only to the host
in `UNEMU_REGISTRY_HOST` (`registry.unemu.dev` when unset). Keep them on the
push step alone, so the import and every other step run without them.

## Publication guard and tags

`unemu image push` runs the publication guard before it uploads anything: a
blob not marked open under a licence goes only to a private namespace or
registry, never into `unemu/`, and an open copyleft medium leaves the private
ones only with its source archive in the same set. There is no override;
`--dry-run` prints the verdict.

Version tags never move: once `unemu/media/example:1.0.0` names an image on
the registry, a push of other bytes under it is refused (a push of the same
image again changes nothing). From v0.0.2 on, the tags of installation media
sets (such as `freedos:1.4` or `1.4-720k`) never move either, not even with
`--move`; publish a changed set under a new revision such as `1.4-r2`.
Aliases such as `latest` move only with `--move`, after a refusal has shown
both digests. `image media import` never moves a tag in the store either.

See `backlog/docs/commands.md` in unemu for the image, media-set and
registry commands.

## Action checks

The action workflow builds the current `unemu`, packages a test release archive,
and substitutes curl's transport to serve that archive. The real installer
checks the SHA-256, the source commit and its PATH output, then runs the installed
binary's version and media creation command. A missing release must leave PATH
and the action output untouched. The release workflow separately checks the
Linux runtime in bare Ubuntu before and after installing ALSA.
