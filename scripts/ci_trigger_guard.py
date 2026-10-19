#!/usr/bin/env python3
"""Fail loudly if a workflow's ``branches:`` filter can never match.

Why this exists
---------------
``quilt-jetson`` shipped a hardcoded "CI: passing" badge over a workflow whose
triggers read::

    on:
      push:         { branches: [main] }
      pull_request: { branches: [main] }

The repository's default branch is ``master`` and no ``main`` branch has ever
existed, so ``ci.yml`` matched no push and no pull request. The workflow could
never fire. The badge was the only thing in the repository claiming CI was
green, and it was claiming something nobody had measured.

A branch filter that cannot match the default branch is therefore a green badge
over zero jobs. That is the whole reason this script exists.

The rule this script enforces
-----------------------------
A ``branches:`` filter is a **union**. Listing several branches adds matches;
it never removes them. So the only way to make a workflow unfireable is to
**omit the default branch**. Accordingly:

1. **FAIL** if any workflow with a branch filter does not cover the default
   branch. This is the "green badge over zero jobs" case.
2. **FAIL** if a filter names a branch that neither exists nor is declared in
   ``.github/branch-aliases.txt``. A phantom name is usually a typo or a stale
   branch rename. The one legitimate case is a forward-compat alias for another
   branch convention, and that has to be declared, reviewed, and visible --
   not assumed by the guard.
3. **FAIL** if the README's CI badge is a hardcoded label, or does not pin the
   default branch.

Two properties this script holds on to, both deliberate:

* **Fail closed.** If the branch list cannot be read, this exits non-zero. A
  guard that reports success because it could not reach the API is the same
  defect wearing a different hat.
* **Measure, do not assume.** The default branch is read from the API on every
  run. It is never hardcoded here, because a hardcoded default branch is the
  same class of bug one level up.

Usage
-----
    python3 scripts/ci_trigger_guard.py              # live check against the API
    python3 scripts/ci_trigger_guard.py --self-test  # offline parser tests
    python3 scripts/ci_trigger_guard.py --repo o/r   # check another repository

Environment: ``GITHUB_TOKEN`` is used if present (only for rate limits; an
unauthenticated read of a public branch list is enough).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.request

DEFAULT_REPO = "SuperInstance/quilt-jetson"
WORKFLOW_DIR = ".github/workflows"
ALIAS_FILE = ".github/branch-aliases.txt"
API = "https://api.github.com"
TIMEOUT = 30

# `on:` / `true:` -- YAML 1.1 parsers turn the bare key `on` into the boolean
# True, so both spellings are accepted when scanning text.
_ON_KEY = re.compile(r"^(on|true):\s*(.*)$")
_EVENT_KEY = re.compile(r"^(\s{2})([A-Za-z_][\w.-]*):\s*(.*)$")
_BRANCHES_KEY = re.compile(r"^(\s{4,})(branches|branches-ignore):\s*(.*)$")
_LIST_ITEM = re.compile(r"^\s*-\s*(.+?)\s*$")


class GuardError(Exception):
    """Raised for anything the guard refuses to guess about."""


def strip_comment(line: str) -> str:
    """Drop a trailing YAML comment. Branch names cannot contain '#'."""
    hash_at = line.find("#")
    return line[:hash_at] if hash_at != -1 else line


def parse_flow_list(raw: str) -> list[str]:
    """Parse ``[a, b]`` into ``['a', 'b']``."""
    inner = raw.strip()
    if inner.startswith("[") and inner.endswith("]"):
        inner = inner[1:-1]
    if not inner.strip():
        return []
    return [item.strip().strip("'\"") for item in inner.split(",") if item.strip()]


def parse_workflow_triggers(text: str) -> dict[str, dict[str, list[str]]]:
    """Extract per-event branch filters from a workflow file.

    Returns ``{event: {'branches': [...], 'branches-ignore': [...]}}``. Events
    without a branch filter are present with empty lists, which is what makes
    "this workflow is unfiltered" distinguishable from "this workflow has a
    filter naming no branch".

    Deliberately a line scanner and not a YAML library: adding a dependency to
    a repository whose CI has just been revealed to be unmeasurable would be a
    poor trade, and the shape of a trigger block is small and stable.
    """
    triggers: dict[str, dict[str, list[str]]] = {}
    lines = text.splitlines()

    on_index = None
    for index, raw_line in enumerate(lines):
        line = strip_comment(raw_line).rstrip()
        match = _ON_KEY.match(line)
        if match:
            on_index = index
            inline = match.group(2).strip()
            if inline:
                # `on: [push, pull_request]` or `on: pull_request`
                names = parse_flow_list(inline) if inline.startswith("[") else [inline]
                for event in names:
                    triggers.setdefault(event, {})
                return triggers
            break

    if on_index is None:
        raise GuardError("no top-level `on:` key found -- is this a workflow file?")

    current_event: str | None = None
    for position in range(on_index + 1, len(lines)):
        raw_line = lines[position]
        if not raw_line.strip():
            continue
        line = strip_comment(raw_line).rstrip()
        if not line.strip():
            continue
        # Left the `on:` block: a new top-level key ends it.
        if not line.startswith(" "):
            break

        branches_match = _BRANCHES_KEY.match(line)
        if branches_match and current_event is not None:
            key, value = branches_match.group(2), branches_match.group(3)
            if value.strip().startswith("["):
                triggers[current_event][key] = parse_flow_list(value)
            else:
                # Block sequence form:
                #   branches:
                #     - master
                block: list[str] = []
                indent = len(branches_match.group(1))
                for follow in lines[position + 1:]:
                    if not follow.strip():
                        continue
                    if not follow.startswith(" "):
                        break
                    stripped = strip_comment(follow).rstrip()
                    item = _LIST_ITEM.match(stripped)
                    if item:
                        if len(follow) - len(follow.lstrip()) <= indent:
                            break
                        block.append(item.group(1).strip("'\""))
                    elif stripped.strip():
                        break
                triggers[current_event][key] = block
            continue

        event_match = _EVENT_KEY.match(line)
        if event_match:
            current_event = event_match.group(2)
            triggers.setdefault(current_event, {})
            inline = event_match.group(3).strip()
            if inline:
                # `push: { branches: [main] }` flow form
                for key, value in re.findall(r"(branches|branches-ignore):\s*(\[[^\]]*\])", inline):
                    triggers[current_event][key] = parse_flow_list(value)
            continue

    return triggers


def parse_alias_file(text: str) -> set[str]:
    """Branch names a workflow may reference before they exist."""
    names: set[str] = set()
    for raw_line in text.splitlines():
        line = strip_comment(raw_line).strip()
        if line:
            names.add(line.strip("'\""))
    return names


def api_get(path: str, token: str | None) -> object:
    request = urllib.request.Request(f"{API}{path}", headers={"Accept": "application/vnd.github+json"})
    if token:
        request.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(request, timeout=TIMEOUT) as response:
            return json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as error:
        raise GuardError(f"GET {path} returned HTTP {error.code} {error.reason}") from error
    except urllib.error.URLError as error:
        raise GuardError(f"GET {path} could not be reached: {error.reason}") from error


def read_default_branch(repo: str, token: str | None) -> str:
    data = api_get(f"/repos/{repo}", token)
    default = data.get("default_branch") if isinstance(data, dict) else None
    if not default:
        raise GuardError(f"the API did not report a default_branch for {repo}")
    return default


def read_branches(repo: str, token: str | None) -> list[str]:
    names: list[str] = []
    page = 1
    while True:
        batch = api_get(f"/repos/{repo}/branches?per_page=100&page={page}", token)
        if not isinstance(batch, list):
            raise GuardError(f"the API did not return a branch list for {repo}")
        if not batch:
            break
        names.extend(item["name"] for item in batch)
        if len(batch) < 100:
            break
        page += 1
    if not names:
        raise GuardError(f"{repo} reported zero branches")
    return names


def check(readme: str, workflows: dict[str, dict[str, dict[str, list[str]]]],
          default_branch: str, existing: list[str],
          aliases: set[str]) -> tuple[list[str], list[str]]:
    """Return ``(failures, warnings)``. An empty ``failures`` means satisfied."""
    failures: list[str] = []
    warnings: list[str] = []
    # Only a workflow that actually CONSTRAINS itself with a `branches:` list can
    # be unfireable. An unfiltered `push:` fires on every branch, and a
    # tag-triggered workflow like release.yml is not gated on branches at all --
    # both are the safe case, and treating them as failures would make this guard
    # cry wolf over its own deliberate design.
    constrained = {
        name: triggers
        for name, triggers in workflows.items()
        if any(keys.get("branches") for keys in triggers.values())
    }

    for name, triggers in sorted(constrained.items()):
        for event, keys in sorted(triggers.items()):
            for wanted in keys.get("branches", []):
                if wanted in existing:
                    continue
                if wanted in aliases:
                    warnings.append(
                        f"{name}: on.{event}.branches names '{wanted}', which does not exist "
                        f"yet; it is a declared forward-compat alias in {ALIAS_FILE} and is "
                        f"harmless because a branch filter is a union"
                    )
                else:
                    failures.append(
                        f"{name}: on.{event}.branches names '{wanted}', which does not exist "
                        f"in the repository and is not a declared alias in {ALIAS_FILE} "
                        f"(default branch is '{default_branch}'; "
                        f"existing branches: {', '.join(sorted(existing))})"
                    )

    if not constrained:
        failures.append(
            "no workflow constrains itself with a `branches:` filter, so nothing asserts "
            f"that CI runs on the default branch '{default_branch}'"
        )
    else:
        for name, triggers in sorted(constrained.items()):
            covered = any(default_branch in keys.get("branches", []) for keys in triggers.values())
            if not covered:
                named = sorted({b for keys in triggers.values() for b in keys.get("branches", [])})
                failures.append(
                    f"{name}: no trigger covers the default branch '{default_branch}', so the "
                    f"filter matches nothing and this workflow can never fire "
                    f"(filter names: {', '.join(named) or 'nothing'})"
                )

    badge_lines = [line.strip() for line in readme.splitlines()
                   if "badge" in line and "actions/workflows" in line]
    if not badge_lines:
        if "img.shields.io/badge/CI-" in readme:
            failures.append(
                "README: the CI badge is a hardcoded shields.io label "
                "(`img.shields.io/badge/CI-...`), which renders the text it is handed and can "
                "never report a failing run. Point it at actions/workflows/ci.yml/badge.svg"
                "?branch=" + default_branch
            )
        else:
            failures.append("README: no CI status badge found")
    else:
        for line in badge_lines:
            if f"?branch={default_branch}" not in line:
                failures.append(f"README: the CI badge does not pin the default branch: {line}")

    return failures, warnings


def self_test() -> int:
    """Offline tests for the parser and every failure condition."""
    failures: list[str] = []
    checked = 0

    def expect(condition: bool, message: str) -> None:
        nonlocal checked
        checked += 1
        if not condition:
            failures.append(message)

    def would_fail(*args: object, **kwargs: object) -> bool:
        return bool(check(*args, **kwargs)[0])

    broken = "on:\n  push:\n    branches: [main]\n  pull_request:\n    branches: [main]\n"
    expect(parse_workflow_triggers(broken) == {"push": {"branches": ["main"]},
                                               "pull_request": {"branches": ["main"]}},
           "flow-list form did not parse")

    fixed = "on:\n  push:\n    branches: [master, main]\n  pull_request:\n    branches: [master, main]\n"
    expect(parse_workflow_triggers(fixed)["push"]["branches"] == ["master", "main"],
           "multi-branch flow list did not parse")

    block = "on:\n  push:\n    branches:\n      - master\n      - main\n"
    expect(parse_workflow_triggers(block)["push"]["branches"] == ["master", "main"],
           f"block-sequence form did not parse: {parse_workflow_triggers(block)}")

    commented = ("on:\n"
                 "  # a comment naming main must not be read as a filter\n"
                 "  push:\n"
                 "    branches: [master]  # trailing comment\n")
    parsed_commented = parse_workflow_triggers(commented)
    expect(parsed_commented == {"push": {"branches": ["master"]}},
           f"comments leaked into the parse: {parsed_commented}")

    unfiltered = parse_workflow_triggers("on: [push, pull_request]\n")
    expect(unfiltered == {"push": {}, "pull_request": {}},
           f"unfiltered form did not parse: {unfiltered}")

    tags_only = parse_workflow_triggers('on:\n  push:\n    tags: ["v*.*.*"]\n')
    expect(tags_only.get("push", {}).get("branches", []) == [],
           f"a tag-only trigger must not read as a branch filter: {tags_only}")

    expect(parse_alias_file("# note\nmain\n\n  release \n") == {"main", "release"},
           "alias file parsing did not strip comments and whitespace")

    badge = ("[![CI](https://github.com/o/r/actions/workflows/ci.yml/badge.svg?branch=master)]"
             "(https://github.com/o/r/actions/workflows/ci.yml)\n")
    good = {"ci.yml": {"push": {"branches": ["master", "main"]},
                       "pull_request": {"branches": ["master", "main"]}}}
    expect(not would_fail(badge, good, "master", ["master"], {"main"}),
           "the corrected repository must PASS")

    expect(would_fail(badge, good, "master", ["main"], {"main"}),
           "a filter that omits the default branch must FAIL")

    phantom = {"ci.yml": {"push": {"branches": ["master", "does-not-exist"]}}}
    expect(would_fail(badge, phantom, "master", ["master"], set()),
           "an undeclared phantom branch must FAIL")

    expect(not would_fail(badge, good, "master", ["master"], {"main"}),
           "a declared forward-compat alias must not FAIL")

    # An unfiltered workflow and a tag-triggered one are the SAFE case: `push:`
    # with no filter fires on every branch. They must not be reported as broken.
    unfiltered_wf = {"ci.yml": {"push": {"branches": ["master"]}},
                     "ci-trigger-guard.yml": {"push": {}, "pull_request": {}},
                     "release.yml": {"push": {}}}
    expect(not would_fail(badge, unfiltered_wf, "master", ["master"], set()),
           f"unfiltered/tag-triggered workflows must not be reported as broken: "
           f"{check(badge, unfiltered_wf, 'master', ['master'], set())[0]}")

    none_constrained = {"ci-trigger-guard.yml": {"push": {}, "pull_request": {}}}
    expect(would_fail(badge, none_constrained, "master", ["master"], set()),
           "a repo where nothing is constrained must FAIL (nothing asserts CI runs)")

    hardcoded = "[![CI](https://img.shields.io/badge/CI-passing-brightgreen)](.)\n"
    expect(would_fail(hardcoded, good, "master", ["master"], set()),
           "a hardcoded 'CI-passing' badge must FAIL")

    stale = ("[![CI](https://github.com/o/r/actions/workflows/ci.yml/badge.svg?branch=main)]"
             "(https://github.com/o/r/actions/workflows/ci.yml)\n")
    expect(would_fail(stale, good, "master", ["master"], {"main"}),
           "a badge pinned to the wrong branch must FAIL")

    if failures:
        print(f"guard self-test FAILED ({checked} assertions, no network):")
        for failure in failures:
            print(f"  - {failure}")
        return 1
    print(f"guard self-test passed ({checked} assertions, no network)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Fail loudly if a workflow's branch filter cannot match.")
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", DEFAULT_REPO))
    parser.add_argument("--root", default=os.environ.get("GITHUB_WORKSPACE", os.getcwd()))
    parser.add_argument("--self-test", action="store_true", help="run offline parser tests and exit")
    args = parser.parse_args()

    if args.self_test:
        return self_test()

    token = os.environ.get("GITHUB_TOKEN") or None

    # Measure. Never assume.
    try:
        default_branch = read_default_branch(args.repo, token)
        existing = read_branches(args.repo, token)
    except GuardError as error:
        print(f"ci-trigger-guard: FAILED TO MEASURE -- {error}", file=sys.stderr)
        print("This guard fails closed on purpose. A guard that passes because it", file=sys.stderr)
        print("could not reach the API is the same defect wearing a different hat.", file=sys.stderr)
        return 2

    workflow_dir = os.path.join(args.root, WORKFLOW_DIR)
    workflows: dict[str, dict[str, dict[str, list[str]]]] = {}
    for filename in sorted(os.listdir(workflow_dir)):
        if not filename.endswith((".yml", ".yaml")):
            continue
        with open(os.path.join(workflow_dir, filename), encoding="utf-8") as handle:
            try:
                workflows[filename] = parse_workflow_triggers(handle.read())
            except GuardError as error:
                print(f"ci-trigger-guard: FAILED -- {filename}: {error}", file=sys.stderr)
                return 1

    with open(os.path.join(args.root, "README.md"), encoding="utf-8") as handle:
        readme = handle.read()

    alias_path = os.path.join(args.root, ALIAS_FILE)
    aliases: set[str] = set()
    if os.path.exists(alias_path):
        with open(alias_path, encoding="utf-8") as handle:
            aliases = parse_alias_file(handle.read())

    print(f"repository        : {args.repo}")
    print(f"default branch    : {default_branch}   (read from the API, not assumed)")
    print(f"branches present  : {', '.join(sorted(existing))}")
    print(f"declared aliases  : {', '.join(sorted(aliases)) or 'none'}")
    for name, triggers in sorted(workflows.items()):
        rendered = {event: keys for event, keys in triggers.items() if keys}
        print(f"workflow          : {name} -> "
              f"{json.dumps(rendered) if rendered else 'no branch filter (fires regardless)'}")

    failures, warnings = check(readme, workflows, default_branch, existing, aliases)
    for warning in warnings:
        print(f"ci-trigger-guard: warning -- {warning}")

    if failures:
        print("")
        print("ci-trigger-guard: FAIL")
        for failure in failures:
            print(f"  - {failure}")
        print("")
        print("A branch filter that cannot match the default branch is a green badge")
        print("over zero jobs. Cover the default branch, or delete the filter entirely.")
        return 1

    print("")
    print(f"ci-trigger-guard: PASS -- every branch filter names a branch that exists or is a "
          f"declared alias, '{default_branch}' is covered, and the README badge points at "
          f"ci.yml on '{default_branch}'.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
