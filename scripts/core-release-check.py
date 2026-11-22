# SPDX-License-Identifier: AGPL-3.0-or-later
import os
import pathlib
import re
import subprocess
import sys
import tempfile

REPOSITORY = "https://github.com/unemu-org/unemupc"


def check(lock, remote):
    packages = [dict(re.findall(r'^([a-z_]+) = "([^"\n]*)"$', block, re.M))
                for block in lock.split("[[package]]")]
    packages = [p for p in packages if p.get("name") == "unemu-pc-core"]
    if len(packages) != 1:
        raise ValueError("Cargo.lock must contain exactly one unemu-pc-core")
    package = packages[0]
    match = re.fullmatch(r"git\+" + re.escape(REPOSITORY)
                         + r"\?rev=([0-9a-f]{40})#\1", package.get("source", ""))
    if not match:
        raise ValueError("unemu-pc-core needs an exact git revision in Cargo.lock")
    revision = match[1]
    refs = dict(line.split()[::-1] for line in subprocess.check_output(
        ["git", "ls-remote", "--tags", remote], text=True,
        stdin=subprocess.DEVNULL, env={**os.environ, "GIT_TERMINAL_PROMPT": "0"}).splitlines())
    # Annotated tags name tag objects; only their peeled commit may match.
    tags = [ref.removeprefix("refs/tags/v") for ref in refs
            if re.fullmatch(r"refs/tags/v[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.+-]+)?", ref)
            and refs.get(ref + "^{}", refs[ref]) == revision]
    if not tags:
        raise ValueError(f"{revision} is not the commit of an unemupc release tag")
    if package["version"] not in tags:
        raise ValueError(f"core release version mismatch: tag(s) {', '.join(tags)}; "
                         f"Cargo.lock unemu-pc-core {package['version']}")
    print(f"core-release-check: {revision} matches unemupc v{package['version']}")


def self_test():
    # Signing, hooks and editors from a contributor's config must not affect fixtures.
    os.environ.update(GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1",
                      GIT_TERMINAL_PROMPT="0", GIT_EDITOR="true", GIT_SEQUENCE_EDITOR="true")
    with tempfile.TemporaryDirectory(prefix="core-release-check-") as directory:
        def git(*args):
            return subprocess.check_output(["git", "-C", directory,
                                            "-c", "user.name=Release test",
                                            "-c", "user.email=test@example.org",
                                            "-c", "commit.gpgsign=false",
                                            "-c", "tag.gpgsign=false",
                                            "-c", "core.editor=true", *args],
                                           text=True, stdin=subprocess.DEVNULL,
                                           stderr=subprocess.DEVNULL).strip()
        git("init", "-q")
        assert not git("config", "--global", "--list"), "self-test read global Git config"
        git("commit", "--allow-empty", "-qm", "release")
        tagged = git("rev-parse", "HEAD")
        git("tag", "-a", "v1.2.3", "-m", "release")
        git("tag", "v1.2.4")
        git("commit", "--allow-empty", "-qm", "development")
        untagged = git("rev-parse", "HEAD")
        for name, revision, version, expected in [
            ("annotated tag", tagged, "1.2.3", None),
            ("lightweight tag", tagged, "1.2.4", None),
            ("untagged", untagged, "1.2.3", "not the commit"),
            ("version mismatch", tagged, "9.9.9", "version mismatch"),
        ]:
            lock = (f'[[package]]\nname = "unemu-pc-core"\nversion = "{version}"\n'
                    f'source = "git+{REPOSITORY}?rev={revision}#{revision}"\n')
            try:
                check(lock, directory)
            except ValueError as error:
                if expected is None or expected not in str(error):
                    raise
                print(f"core-release-check: self-test {name} rejected: {error}")
            else:
                if expected:
                    raise AssertionError(f"{name} was accepted")
        for name, lock, expected in [
            ("local checkout", '[[package]]\nname = "unemu-pc-core"\nversion = "1.2.3"\n',
             "needs an exact git revision"),
            ("missing core", "version = 4\n", "exactly one unemu-pc-core"),
        ]:
            try:
                check(lock, directory)
            except ValueError as error:
                assert expected in str(error), error
                print(f"core-release-check: self-test {name} rejected: {error}")
            else:
                raise AssertionError(f"{name} was accepted")
        print("core-release-check: self-test passed (2 tagged matches; untagged, version mismatch, local checkout and missing core rejected)")


try:
    if sys.argv[2] == "--self-test":
        self_test()
    elif sys.argv[2]:
        raise ValueError("usage: core-release-check.sh [--self-test]")
    else:
        check(pathlib.Path(sys.argv[1]).read_text(), REPOSITORY)
except (ValueError, subprocess.CalledProcessError) as error:
    sys.exit(f"core-release-check: {error}")
