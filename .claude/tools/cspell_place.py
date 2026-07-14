#!/usr/bin/env python3
"""cspell word-placement helper — decide, for each new cspell-unknown word,
whether it belongs in the project dictionary (`cfg/dictionary.txt`) or in a
per-file inline `cspell:word` escape, per the ≥2-file rule in
`docs/conventions/docs-and-style.md`.

Placing a new word by hand drives a loop of `pre-commit run cspell` round-trips,
each iteration re-deciding dictionary-vs-inline and occasionally slipping a word
to CI (an edit re-ran through one hook but not cspell). This tool collapses that
loop to one call: it counts each word's spread across the repo and prints the
verdict — the rule (a word in **≥ 2 files** goes in the dictionary; a word in
**one** file gets an inline escape in that file, in that file's comment style;
the lone exception is a file that can't carry a comment, e.g. `.json`, which
falls back to the dictionary).

Two subcommands:

* ``verdict WORD... [--changed FILE...]`` — the deterministic core: for each
  given word, print where it should live. ``--changed`` biases the single-file
  target toward a file being edited when the repo count is ambiguous.
* ``scan --files FILE...`` — run cspell over the changed files to list the
  unknown words first, then verdict them (best-effort; needs the cspell CLI,
  invocation overridable with ``--cspell``).

Word spread is counted by a light tokenizer (letter runs + camelCase splits,
compared case-insensitively) — an approximation of cspell's own tokenization,
good enough to steer placement. Stdlib only; a Python skill-tool under
`.claude/tools/` — deliberately **not** a Cargo workspace member (see
``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

DICTIONARY = "cfg/dictionary.txt"
CSPELL_CONFIG = "cfg/cspell.yml"

# Inline-escape comment style per file extension. ``None`` = the file can't
# carry a comment, so the dictionary is the only option (docs-and-style rule).
_LINE_COMMENT = {
    ".rs": "// cspell:word {word}",
    ".ts": "// cspell:word {word}",
    ".tsx": "// cspell:word {word}",
    ".js": "// cspell:word {word}",
    ".jsx": "// cspell:word {word}",
    ".md": "<!-- cspell:word {word} -->",
    ".yml": "# cspell:word {word}",
    ".yaml": "# cspell:word {word}",
    ".toml": "# cspell:word {word}",
    ".sh": "# cspell:word {word}",
    ".py": "# cspell:word {word}",
}
_NO_COMMENT = {".json"}

_WORD_RUN_RE = re.compile(r"[A-Za-z]+")
_CAMEL_RE = re.compile(r"[A-Z]+(?=[A-Z][a-z])|[A-Z]?[a-z]+|[A-Z]+")


def comment_style(path: str) -> str | None:
    """The inline-escape directive template for a file, or ``None`` if the file
    can't carry a comment (so the dictionary is the only home)."""
    ext = Path(path).suffix.lower()
    if ext in _NO_COMMENT:
        return None
    return _LINE_COMMENT.get(ext)


def tokenize(text: str) -> set[str]:
    """Lowercased word tokens in ``text`` — each letter run, plus its camelCase
    sub-parts, so ``fooBar`` yields ``foobar`` / ``foo`` / ``bar``."""
    tokens: set[str] = set()
    for run in _WORD_RUN_RE.findall(text):
        tokens.add(run.lower())
        for part in _CAMEL_RE.findall(run):
            if part:
                tokens.add(part.lower())
    return tokens


def load_dictionary(path: Path) -> set[str]:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError:
        return set()
    return {ln.strip().lower() for ln in lines if ln.strip() and not ln.startswith("#")}


def _read_text(path: Path) -> str | None:
    try:
        return path.read_text(encoding="utf-8")
    except (OSError, ValueError, UnicodeDecodeError):
        return None


def count_word_files(
    words: list[str], files: list[str], dictionary_path: str = DICTIONARY
) -> dict:
    """Map each (lowercased) word to the sorted list of files whose tokens
    include it. The dictionary file itself is excluded — a dictionary entry is
    not a *usage*."""
    wanted = {w.lower() for w in words}
    hits: dict = {w: set() for w in wanted}
    dict_norm = str(Path(dictionary_path))
    for f in files:
        if str(Path(f)) == dict_norm:
            continue
        text = _read_text(Path(f))
        if text is None:
            continue
        tokens = tokenize(text)
        for w in wanted:
            if w in tokens:
                hits[w].add(f)
    return {w: sorted(fs) for w, fs in hits.items()}


def verdict(
    word: str, files_with_word: list[str], changed: list[str], dictionary: set[str]
) -> dict:
    """Where a single word should live."""
    lw = word.lower()
    if lw in dictionary:
        return {
            "word": word,
            "placement": "already-in-dictionary",
            "files": files_with_word,
        }

    n = len(files_with_word)
    if n >= 2:
        return {
            "word": word,
            "placement": "dictionary",
            "reason": f"used in {n} files (≥2)",
            "files": files_with_word,
            "target": DICTIONARY,
        }

    # One file (or, defensively, zero repo hits → fall back to a changed file):
    target = (
        files_with_word[0] if files_with_word else (changed[0] if changed else None)
    )
    if target is None:
        return {"word": word, "placement": "unknown", "reason": "no file located"}

    style = comment_style(target)
    if style is None:
        # Lone exception: a file that can't carry a comment → dictionary.
        return {
            "word": word,
            "placement": "dictionary",
            "reason": f"only in {target}, which can't carry a comment",
            "files": files_with_word,
            "target": DICTIONARY,
        }
    return {
        "word": word,
        "placement": "inline",
        "reason": f"used in 1 file ({target})",
        "target": target,
        "directive": style.format(word=word),
    }


def _git_ls_files(git=None) -> list[str]:
    runner = git or (
        lambda args: subprocess.run(
            ["git", *args], capture_output=True, text=True, check=False
        )
    )
    proc = runner(["ls-files"])
    if getattr(proc, "returncode", 1) != 0:
        raise RuntimeError("git ls-files failed")
    return [ln for ln in proc.stdout.splitlines() if ln.strip()]


def run_verdicts(words: list[str], changed: list[str], repo_files: list[str]) -> dict:
    dictionary = load_dictionary(Path(DICTIONARY))
    hits = count_word_files(words, repo_files)
    results = [verdict(w, hits[w.lower()], changed, dictionary) for w in words]
    return {"words": results}


def _run_cspell(files: list[str], cspell_cmd: str) -> list[str]:
    """List cspell-unknown words over ``files`` via ``--words-only --unique``."""
    cmd = [
        *cspell_cmd.split(),
        "--config",
        CSPELL_CONFIG,
        "--words-only",
        "--unique",
        "--no-progress",
        "--no-summary",
        *files,
    ]
    proc = subprocess.run(cmd, capture_output=True, text=True, check=False)
    # cspell exits non-zero when it finds unknown words — that's the signal, not
    # an error. Words are printed one per line on stdout.
    seen: list[str] = []
    for ln in proc.stdout.splitlines():
        w = ln.strip()
        if w and w not in seen:
            seen.append(w)
    return seen


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="cspell_place.py")
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_v = sub.add_parser("verdict", help="place explicit words")
    p_v.add_argument("words", nargs="+")
    p_v.add_argument("--changed", nargs="*", default=[])

    p_s = sub.add_parser("scan", help="run cspell over files, then place the words")
    p_s.add_argument("--files", nargs="+", required=True)
    p_s.add_argument("--cspell", default="cspell", help="cspell CLI invocation")

    args = parser.parse_args(argv[1:])
    repo_files = _git_ls_files()

    if args.cmd == "verdict":
        result = run_verdicts(args.words, args.changed, repo_files)
    else:
        words = _run_cspell(args.files, args.cspell)
        result = run_verdicts(words, args.files, repo_files)
        result["scanned_files"] = args.files

    json.dump(result, sys.stdout, indent=2, ensure_ascii=False)
    sys.stdout.write("\n")
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except (RuntimeError, OSError) as e:
        print(f"error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
