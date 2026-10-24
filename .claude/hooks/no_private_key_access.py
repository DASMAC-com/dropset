#!/usr/bin/env python3
# cspell:word caffeinate
# cspell:word commenters
# cspell:word execdir
# cspell:word gsed
# cspell:word okdir
# cspell:word procs
# cspell:word sshlogin
"""PreToolUse guard: keep every agent session away from the private keys.

The mainnet private keys live in a 1Password vault and sign through the
1Password SSH agent. No agent session or sub-agent ever needs them, and a
private key cannot be rotated once it leaks: the address it controls is the
identity. So this guard denies, with **no escape marker**, every tool call
that reaches toward them:

1. **Names.** Any Bash command naming the private-keys vault (its name or id,
   read from ``DS_OP_PRIVATE_KEYS_VAULT`` in the session environment, comma
   separated for both; unset, the vault check is off), an ``op://``
   reference to one of the key items (``eBook``, ``Admin``, ``Lead``,
   ``Quote`` and their signing duplicates — matched on the item's first
   word), or the 1Password agent socket. The committed script carries no real
   vault name. The bare titles are matched only inside an ``op://``
   reference: ``Admin`` and ``Quote`` are ordinary words in this codebase,
   and a guard that blocks ``grep Quote`` gets turned off. Fetching an item
   by its bare title needs ``op``, which rule 2 blocks anyway.
2. **``op`` at any command position**, including after ``env``, ``sudo``,
   ``xargs``, ``watch``, ``find -exec``, a control operator (glued or
   spaced), inside a ``-c`` payload, ``eval``, a command substitution, or a
   shell reading a here-string or a pipe — the shapes a ``Bash(op:*)``
   prefix deny rule cannot see. The make targets carry the ``op run`` the
   collectors and the keyed frontend need, so an agent never types ``op``.
3. **The agent and session handles.** Setting ``SSH_AUTH_SOCK``,
   ``OP_SESSION_*`` or ``OP_SERVICE_ACCOUNT_TOKEN`` (an assignment in
   command position or under ``env`` / ``export`` / ``declare``, or a
   ``printf -v`` / ``read`` target), ``ssh-add``, ``ssh-keygen -Y sign``,
   and the ``--session`` flag. A *mention* (``grep SSH_AUTH_SOCK=``) passes.
4. **The mainnet cockpit.** ``make tui-mainnet``; ``make teardown`` (or
   ``dropset-teardown``) with a non-localnet ``--rpc-url``, whether ``ARGS``
   is a make argument or an environment assignment; ``dropset-tui``, or any
   ``cargo run`` / ``cargo r``, with ``--cluster mainnet`` or
   ``--keys-file``. ``make mainnet-demo`` (watch-only, ephemeral key) stays
   allowed here; the binary itself refuses an agent shell on mainnet. An RPC
   URL carrying userinfo (``http://localhost@host``) is never localnet.
5. **The four reference files** — ``tui/private-keys.env``,
   ``infra/localnet/secrets.local.env``, ``frontend/.env.local`` and
   ``~/.config/1Password/ssh/agent.toml`` — may not be mutated, through
   ``Edit`` / ``Write`` / ``MultiEdit`` or through Bash (a redirection, or a
   mutating command such as ``rm``, ``mv``, ``cp``, ``ln``, ``tee`` or
   ``sed -i`` naming one, a directory destination included). A path is
   matched by suffix after ``~`` / ``$HOME`` expansion and resolution
   against the checkout, so the base copy, every worktree's copy, and a
   ``../../../`` spelling all match. Reading them is allowed: the env files
   hold ``op://`` references and ``agent.toml`` names which keys the agent
   offers; none holds key material.

The ``permissions.deny`` rules in the user-local settings are the other
layer (``docs/conventions/local-integrations.md`` → "The private-key guard
hook"): they hold under auto mode with no script running. This hook runs
first, so its message is what the agent reads, and it covers the shapes a
prefix rule cannot express.

**What neither layer can stop**: a script whose contents a prefix rule and
this tokenizer never see (``make`` running a recipe, ``python3 x.py``),
variable indirection (``P=op; $P read``), a symlinked spelling of a
reference file, a recursive delete of a parent directory, an MCP tool that
shells out, and a sub-agent or session without the wiring. The layer that
holds there is the 1Password SSH agent's own per-key prompt: a signing
request from any process names the key and the requesting process, and the
operator refuses it.

**Parsing.** The whole command is tokenized once, quote-aware, with an
unquoted newline as a command separator — so a quoted string spanning lines
(a commit message describing this guard) stays one word, and a
backslash-newline continuation joins its lines. Runs of glued operators
(``;(``, ``&&(``, ``;>``) are split into their parts. Only when the whole
command's quotes do not balance does it fall back to scanning line by line,
skipping a line that does not parse. The name checks (rule 1) run on the raw
text first and need no parse.

Reads the PreToolUse payload as JSON on stdin; exits 2 with the reason on
stderr to block. Fails **open** on an unparseable payload or an internal
error. Run the built-in cases with ``--self-test``;
``.claude/tools/tests/test_no_private_key_access.py`` pins them under
``make tools-tests``.
"""

import json
import os
import re
import shlex
import sys

VAULT_ENV = "DS_OP_PRIVATE_KEYS_VAULT"

# A configured vault value shorter than this is ignored rather than matched as
# a substring of every command — a mis-set `a` must not wedge the session.
MIN_VAULT_LEN = 3

# The key items, matched case-insensitively on the first word of an
# `op://<vault>/<item>/…` reference's item segment, so `Admin signing` and
# `Quote-ssh` match too.
KEY_ITEMS = frozenset({"ebook", "admin", "lead", "quote"})

OP_REF_RE = re.compile(r"op://([^/\s'\"]+)/([^/\s'\"]+)")
ITEM_WORD_RE = re.compile(r"[A-Za-z0-9]+")

# The agent socket is `agent.sock` under 1Password's macOS group container,
# whose directory starts with the app's team id.
SOCKET_FILE = "agent.sock"
SOCKET_MARKERS = ("1password", "2bua8c4s2c")

# The variables whose value hands a process a key-signing or vault session.
HANDLE_NAME_RE = re.compile(
    r"^(SSH_AUTH_SOCK|OP_SESSION_[A-Za-z0-9_]*|OP_SERVICE_ACCOUNT_TOKEN)$"
)
ASSIGN_RE = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)\+?=")
# Builtins whose operands are assignments (`export X=1`) or, for the last two,
# bare variable names they set (`printf -v X …`, `read X`).
DECLARERS = frozenset({"export", "declare", "typeset", "local", "readonly"})
NAME_SETTERS = frozenset({"printf", "read"})

# The reference files as lowercased path suffixes, each with the name the
# deny message shows.
REFERENCE_FILES = {
    "tui/private-keys.env": "tui/private-keys.env",
    "infra/localnet/secrets.local.env": "infra/localnet/secrets.local.env",
    "frontend/.env.local": "frontend/.env.local",
    ".config/1password/ssh/agent.toml": "~/.config/1Password/ssh/agent.toml",
}
HOME_VAR_RE = re.compile(r"^\$(\{HOME\}|HOME)(?=/|$)")

EDIT_TOOLS = ("Edit", "Write", "MultiEdit")

# Shell operator characters, and every operator they spell, longest first so a
# glued run (`;(`, `&&(`, `;>`) splits into its real parts.
OPERATOR_CHARS = "();<>|&\n"
OPERATORS = sorted(
    (
        "&&", "||", "|&", ";;", "&>>", "&>", ">&", "<&", ">>", ">|", "<>",
        "<<<", "<<", "<(", ">(", "|", "&", ";", "(", ")", "<", ">", "\n",
    ),
    key=len,
    reverse=True,
)  # fmt: skip

# Tokens after which the next word is a command.
CONTROL = frozenset(
    {"|", "||", "&&", ";", "&", "|&", ";;", "(", ")", "<(", ">(", "\n", "{", "}", "!"}
)
KEYWORDS = frozenset({"then", "do", "else", "elif", "if", "while", "until"})
# Output redirects, whose target is written; and the input forms, whose
# operand is skipped when reading a command's words.
REDIRECTS = frozenset({">", ">>", ">|", "&>", "&>>", "<>", ">&"})
INPUTS = frozenset({"<", "<<", "<&"})
HERE_STRING = "<<<"

SHELLS = frozenset({"sh", "bash", "zsh", "dash", "ksh", "fish"})

# Wrappers that run their trailing words as a command, with the flags of each
# that consume the following token.
WRAPPERS = {
    "env": frozenset({"-u", "--unset", "-C", "--chdir"}),
    "sudo": frozenset({"-u", "-g", "-h", "-C", "-p", "-r", "-t", "-U", "-D"}),
    "doas": frozenset({"-u", "-C"}),
    "nohup": frozenset(),
    "exec": frozenset({"-a"}),
    "builtin": frozenset(),
    "nice": frozenset({"-n"}),
    "caffeinate": frozenset({"-w", "-t"}),
    "stdbuf": frozenset({"-i", "-o", "-e"}),
    "time": frozenset(),
    "timeout": frozenset({"-s", "--signal", "-k", "--kill-after"}),
    "xargs": frozenset(
        {
            "-I",
            "-L",
            "-n",
            "-P",
            "-s",
            "-d",
            "-E",
            "-a",
            "--max-args",
            "--max-lines",
            "--max-procs",
            "--max-chars",
            "--delimiter",
            "--eof",
            "--arg-file",
            "--replace",
        }
    ),  # fmt: skip
    "parallel": frozenset({"-j", "--jobs", "-S", "--sshlogin"}),
}

# Commands that mutate every file they name, and those that write to a
# destination operand (copying *from* a reference file is a read; moving one
# away is not).
MUTATE_ALL = frozenset(
    {"rm", "tee", "truncate", "touch", "chmod", "chown", "shred", "unlink"}
)
MUTATE_DEST = frozenset({"mv", "cp", "ln", "rsync", "install", "ditto"})
IN_PLACE = frozenset({"sed", "gsed", "perl"})

LOCAL_HOSTS = frozenset({"localhost", "127.0.0.1", "0.0.0.0", "::1", "[::1]"})
LOCAL_MONIKERS = frozenset({"l", "localhost", "localnet"})

MAX_DEPTH = 6

DENY_MESSAGE = """\
BLOCKED: PRIVATE KEYS AND MAINNET ARE OFF LIMITS TO AGENTS.

What tripped: {reason}.

The mainnet private keys are off limits to every agent session and every
sub-agent, without exception, and an agent shell never runs mainnet
operations. A private key cannot be rotated once it leaks: the address it
controls IS the identity, so one exposure is permanent.

Stop here. Do not route around this block: do not rephrase the command,
split it, move it into a script or a make recipe, or hand it to a
sub-agent. Tell the operator, in plain words, what step needs a private key
or mainnet, and let them run it in their own terminal. This guard has no
escape marker by design."""


def _vaults(env):
    raw = env.get(VAULT_ENV) or ""
    if not isinstance(raw, str):
        return []
    return [
        v.strip().lower() for v in raw.split(",") if len(v.strip()) >= MIN_VAULT_LEN
    ]


def _name_reason(text, vaults):
    """Rule 1 on raw text: the vault, a key item reference, the agent socket."""
    low = text.lower()
    for vault in vaults:
        if vault in low:
            return "the command names the private-keys vault"
    for vault_seg, item_seg in OP_REF_RE.findall(text):
        if vault_seg.lower() in vaults:
            return "the command names the private-keys vault"
        words = ITEM_WORD_RE.findall(item_seg)
        if words and words[0].lower() in KEY_ITEMS:
            return "the command references a private-key item (%s)" % item_seg
    if SOCKET_FILE in low and any(m in low for m in SOCKET_MARKERS):
        return "the command names the 1Password SSH agent socket"
    return None


def _lex(text):
    lex = shlex.shlex(text, posix=True, punctuation_chars=OPERATOR_CHARS)
    lex.whitespace = " \t\r"
    lex.whitespace_split = True
    lex.commenters = ""
    return list(lex)


def _split_operators(tok):
    """A token made only of operator characters, split into its operators."""
    if not tok or any(c not in OPERATOR_CHARS for c in tok):
        return [tok]
    parts = []
    i = 0
    while i < len(tok):
        op = next((o for o in OPERATORS if tok.startswith(o, i)), tok[i])
        parts.append(op)
        i += len(op)
    return parts


def _tokenize(command):
    """Quote-aware tokens with an unquoted newline as its own `\\n` token.

    Falls back to line-by-line, skipping a line that does not parse, only when
    the whole command's quotes do not balance.
    """
    text = command.replace("\x00", "").replace("\\\n", "")
    # ANSI-C and locale quoting (`$'op'`) quote exactly like plain quotes for
    # the words this guard reads.
    text = re.sub(r"\$(?=['\"])", "", text)
    try:
        raw = _lex(text)
    except ValueError:
        raw = []
        for line in text.split("\n"):
            try:
                raw.extend(_lex(line))
            except ValueError:
                pass
            raw.append("\n")
    tokens = []
    for tok in raw:
        tokens.extend(_split_operators(tok))
    return tokens


def _segments(tokens):
    """Split tokens into commands at control operators."""
    segments = [[]]
    for tok in tokens:
        if tok in CONTROL:
            segments.append([])
        else:
            segments[-1].append(tok)
    return [s for s in segments if s]


def _resolve(path, cwd):
    path = os.path.expanduser(HOME_VAR_RE.sub("~", path))
    if not os.path.isabs(path):
        path = os.path.join(cwd, path)
    return os.path.normpath(path)


def _reference_file(path, cwd):
    """The display name of the reference file a path names, or None."""
    if not isinstance(path, str) or not path:
        return None
    low = _resolve(path, cwd).lower()
    for suffix, name in REFERENCE_FILES.items():
        if low.endswith("/" + suffix):
            return name
    return None


def _rpc_urls(args):
    urls = []
    for i, tok in enumerate(args):
        if tok in ("--rpc-url", "-u", "--url") and i + 1 < len(args):
            urls.append(args[i + 1])
        elif tok.startswith("--rpc-url="):
            urls.append(tok.split("=", 1)[1])
    return urls


def _is_local(url):
    low = url.lower()
    if low in LOCAL_MONIKERS:
        return True
    rest = re.sub(r"^[a-z][a-z0-9+.-]*://", "", low)
    authority = re.split(r"[/?#]", rest, maxsplit=1)[0]
    if "@" in authority:
        # Userinfo: `http://localhost:8899@mainnet-host` connects to the host
        # after the `@`. Never call that localnet.
        return False
    if authority.startswith("["):
        host = authority.split("]", 1)[0] + "]"
    elif authority.count(":") == 1:
        host = authority.split(":", 1)[0]
    else:
        host = authority
    return host in LOCAL_HOSTS


def _cockpit_reason(args):
    """Rule 4 over a cockpit's own arguments."""
    for i, tok in enumerate(args):
        value = None
        if tok == "--cluster" and i + 1 < len(args):
            value = args[i + 1]
        elif tok.startswith("--cluster="):
            value = tok.split("=", 1)[1]
        if value is not None and value.lower().startswith("mainnet"):
            return "the TUI cockpit pointed at mainnet (--cluster %s)" % value
        if tok == "--keys-file" or tok.startswith("--keys-file="):
            return "the TUI cockpit given a private-keys file (--keys-file)"
    return None


def _teardown_reason(args):
    for url in _rpc_urls(args):
        if not _is_local(url):
            return "teardown pointed at a non-localnet RPC (%s)" % url
    return None


def _split_args(value):
    try:
        return shlex.split(value)
    except ValueError:
        return value.split()


def _make_reason(args, env_args):
    """Rule 4 for make. ``env_args`` is any ``ARGS=`` set outside the call."""
    targets = []
    make_args = list(env_args)
    i = 0
    while i < len(args):
        tok = args[i]
        if tok in ("-C", "-f", "-I", "-o", "-W", "--directory", "--file"):
            i += 2
            continue
        if tok.startswith("-"):
            i += 1
            continue
        if tok.startswith("ARGS="):
            make_args.extend(_split_args(tok.split("=", 1)[1]))
        elif "=" not in tok:
            targets.append(tok)
        i += 1
    if "tui-mainnet" in targets:
        return "`make tui-mainnet`, the mainnet cockpit"
    if "teardown" in targets:
        reason = _teardown_reason(make_args)
        if reason:
            return reason
    return _cockpit_reason(make_args)


def _cargo_reason(args):
    """Rule 4 for cargo: any `cargo run` / `cargo r`, whatever the package.

    `--cluster mainnet` and `--keys-file` mean the cockpit in every binary
    this repo has, so the package spelling (`-p`, `--manifest-path`, a cwd
    inside `tui/`) does not matter.
    """
    subcommand = next((a for a in args if not a.startswith("-")), None)
    if subcommand not in ("run", "r"):
        return None
    for i, tok in enumerate(args):
        binary = None
        if tok == "--bin" and i + 1 < len(args):
            binary = args[i + 1]
        elif tok.startswith("--bin="):
            binary = tok.split("=", 1)[1]
        if binary == "dropset-teardown":
            reason = _teardown_reason(args)
            if reason:
                return reason
    return _cockpit_reason(args)


def _in_place_flag(name, arg):
    """`-i`, `--in-place`, or a short-flag cluster carrying `i` (`perl -pi`).

    A perl `-M` / `-m` / `-I` / `-x` switch takes the rest of its token as a
    value (`-Mstrict`), so it is never a cluster.
    """
    if arg.startswith("--"):
        return arg.startswith("--in-place")
    if not arg.startswith("-") or len(arg) < 2:
        return False
    if name == "perl" and arg[1] in "MmIx":
        return False
    letters = re.match(r"[A-Za-z]*", arg[1:]).group(0)
    return "i" in letters


def _mutation_candidates(name, args):
    """The paths a mutating command would write, directory destinations
    resolved to the file they would create."""
    target_dir = None
    operands = []
    i = 0
    while i < len(args):
        tok = args[i]
        if tok in ("-t", "--target-directory") and i + 1 < len(args):
            target_dir = args[i + 1]
            i += 2
            continue
        if tok.startswith("--target-directory="):
            target_dir = tok.split("=", 1)[1]
        elif not tok.startswith("-") or tok == "-":
            operands.append(tok)
        i += 1
    if name in MUTATE_ALL:
        return operands
    if name in IN_PLACE:
        return operands if any(_in_place_flag(name, a) for a in args) else []
    if name == "dd":
        return [a[3:] for a in args if a.startswith("of=")]
    if name not in MUTATE_DEST:
        return []
    if target_dir is not None:
        destinations, sources = [target_dir], operands
    else:
        destinations, sources = operands[-1:], operands[:-1]
    candidates = list(destinations)
    for dest in destinations:
        for source in sources:
            candidates.append(os.path.join(dest, os.path.basename(source.rstrip("/"))))
    if name == "mv":
        candidates.extend(sources)
    return candidates


def _unwrap(seg):
    """Strip redirects, keywords, assignments and wrappers.

    Returns ``(argv, nested, assigns, reads_stdin)``: ``nested`` holds command
    strings to scan recursively (a `-c` payload, an `env -S` string, `eval`'s
    or `watch`'s words, a here-string fed to a shell); ``assigns`` every
    variable assignment the command makes; ``reads_stdin`` is True for a
    shell given no command to run, so it executes whatever is piped in.
    """
    words = []
    here = []
    i = 0
    while i < len(seg):
        tok = seg[i]
        if tok == HERE_STRING:
            here.extend(seg[i + 1 : i + 2])
            i += 2
            continue
        if tok in REDIRECTS or tok in INPUTS:
            i += 2
            continue
        words.append(tok)
        i += 1

    nested = []
    assigns = []
    i = 0
    while i < len(words) and (words[i] in KEYWORDS or ASSIGN_RE.match(words[i])):
        if ASSIGN_RE.match(words[i]):
            assigns.append(words[i])
        i += 1
    argv = words[i:]
    while argv:
        name = os.path.basename(argv[0])
        if name == "command":
            j = 1
            while j < len(argv) and argv[j].startswith("-"):
                if "v" in argv[j] or "V" in argv[j]:
                    # `command -v op` looks a name up; it runs nothing.
                    return [], nested, assigns, False
                j += 1
            argv = argv[j:]
            continue
        if name == "watch":
            j = 1
            while j < len(argv) and argv[j].startswith("-"):
                j += 2 if argv[j] in ("-n", "--interval") else 1
            # watch hands its words to `sh -c`.
            nested.append(" ".join(argv[j:]))
            return [], nested, assigns, False
        if name not in WRAPPERS:
            break
        value_flags = WRAPPERS[name]
        j = 1
        positional_skipped = False
        while j < len(argv):
            tok = argv[j]
            if name == "env" and tok in ("-S", "--split-string"):
                nested.extend(argv[j + 1 : j + 2])
                return [], nested, assigns, False
            if tok == "--":
                j += 1
                break
            if tok in value_flags:
                j += 2
                continue
            if tok.startswith("-"):
                j += 1
                continue
            if ASSIGN_RE.match(tok) and name in ("env", "sudo", "doas"):
                assigns.append(tok)
                j += 1
                continue
            if name == "timeout" and not positional_skipped:
                positional_skipped = True
                j += 1
                continue
            break
        argv = argv[j:]
    if not argv:
        return [], nested, assigns, False

    name = os.path.basename(argv[0])
    reads_stdin = False
    if name in SHELLS:
        payload = None
        operand = False
        for j, tok in enumerate(argv[1:], start=1):
            if tok.startswith("-") and not tok.startswith("--") and "c" in tok[1:]:
                payload = argv[j + 1] if j + 1 < len(argv) else None
                break
            if not tok.startswith("-"):
                operand = True
                break
        if payload is not None:
            nested.append(payload)
        elif not operand:
            reads_stdin = True
            nested.extend(here)
    elif name == "eval":
        nested.append(" ".join(argv[1:]))
    elif name == "find":
        for j, tok in enumerate(argv):
            if tok in ("-exec", "-execdir", "-ok", "-okdir"):
                nested.append(shlex.join(argv[j + 1 :]))
    elif name in DECLARERS:
        assigns.extend(a for a in argv[1:] if ASSIGN_RE.match(a))
    elif name in NAME_SETTERS:
        assigns.extend(a + "=" for a in argv[1:] if HANDLE_NAME_RE.match(a))
    return argv, nested, assigns, reads_stdin


def _command_reason(argv, assigns, env_args, cwd):
    for assign in assigns:
        var = ASSIGN_RE.match(assign).group(1)
        if HANDLE_NAME_RE.match(var):
            return "an assignment to %s" % var
    if not argv:
        return None
    name = os.path.basename(argv[0])
    args = argv[1:]
    if name == "op":
        return "a direct `op` (1Password CLI) invocation"
    if name == "ssh-add":
        return "`ssh-add`, which loads or lists signing keys in an SSH agent"
    if name == "ssh-keygen":
        for i, tok in enumerate(args):
            nxt = args[i + 1] if i + 1 < len(args) else ""
            if tok == "-Ysign" or (tok == "-Y" and nxt == "sign"):
                return "`ssh-keygen -Y sign`, a signing request to the key agent"
    if name in ("make", "gmake"):
        return _make_reason(args, env_args)
    if name == "dropset-tui":
        return _cockpit_reason(args)
    if name == "dropset-teardown":
        return _teardown_reason(args)
    if name == "cargo":
        return _cargo_reason(args)
    for path in _mutation_candidates(name, args):
        ref = _reference_file(path, cwd)
        if ref:
            return "`%s` would modify the reference file %s" % (name, ref)
    return None


def _token_reason(tokens, cwd):
    """Checks that look at every token, whatever its position."""
    for i, tok in enumerate(tokens):
        if tok == "--session" or tok.startswith("--session="):
            return "the `--session` flag (a 1Password session token)"
        if tok in REDIRECTS and i + 1 < len(tokens):
            ref = _reference_file(tokens[i + 1], cwd)
            if ref:
                return "a redirection that would overwrite the reference file %s" % ref
    return None


def _scan(command, cwd, depth=0):
    if depth > MAX_DEPTH:
        return None
    tokens = _tokenize(command)
    reason = _token_reason(tokens, cwd)
    if reason:
        return reason
    env_args = []
    for tok in tokens:
        if tok.startswith("ARGS="):
            env_args.extend(_split_args(tok.split("=", 1)[1]))
    nested = []
    for tok in tokens:
        if "$(" in tok:
            nested.append(tok.split("$(", 1)[1])
        elif "`" in tok:
            nested.append(tok.replace("`", " ; "))
    stdin_shell = False
    for seg in _segments(tokens):
        argv, inner, assigns, reads_stdin = _unwrap(seg)
        nested.extend(inner)
        stdin_shell = stdin_shell or reads_stdin
        reason = _command_reason(argv, assigns, env_args, cwd)
        if reason:
            return reason
    if stdin_shell:
        # A shell reading its commands from a pipe runs words this scan saw
        # only as arguments (`echo 'op read x' | sh`).
        nested.extend(t for t in tokens if " " in t or "\n" in t)
    for payload in nested:
        if payload and payload != command:
            reason = _scan(payload, cwd, depth + 1)
            if reason:
                return reason
    return None


def _evaluate(payload, env, cwd):
    if not isinstance(payload, dict):
        return 0, ""
    tool = payload.get("tool_name")
    tool_input = payload.get("tool_input") or {}
    if not isinstance(tool_input, dict):
        return 0, ""
    for candidate in (cwd, payload.get("cwd"), env.get("CLAUDE_PROJECT_DIR")):
        if isinstance(candidate, str) and candidate:
            cwd = candidate
            break
    else:
        cwd = os.getcwd()
    reason = None
    if tool in EDIT_TOOLS:
        ref = _reference_file(tool_input.get("file_path"), cwd)
        if ref:
            reason = "`%s` of the reference file %s" % (tool, ref)
    elif tool == "Bash":
        cmd = tool_input.get("command", "")
        if isinstance(cmd, str) and cmd.strip():
            reason = _name_reason(cmd, _vaults(env)) or _scan(cmd, cwd)
    if reason:
        return 2, DENY_MESSAGE.format(reason=reason)
    return 0, ""


def evaluate(payload, env=None, cwd=None):
    """Return (exit_code, message). exit_code 2 blocks; 0 allows.

    Any internal error allows, like every guard here: a crash must never
    wedge the session.
    """
    env = os.environ if env is None else env
    try:
        return _evaluate(payload, env, cwd)
    except Exception:
        return 0, ""


# (command, should_block). The self-test's vault is a placeholder: the real
# name lives only in the operator's runtime init.
TEST_VAULT = "Example Keys Vault,abc123-vault-id"
TEST_CWD = "/Users/x/repos/dropset/.claude/worktrees/eng-1"
BASH_CASES = [
    # Rule 1: names.
    ("grep -r 'Example Keys Vault' .", True),
    ("echo ABC123-VAULT-ID", True),
    ("cat notes | grep 'op://Some/Admin/private key'", True),
    ("echo op://Other/Quote-signing/key", True),
    ("ls ~/Library/Group\\ Containers/2BUA8C4S2C.com.1password/t/agent.sock", True),
    ("ls ~/.1password/agent.sock", True),
    (
        "ssh -o IdentityAgent=~/Library/Group\\ Containers/2BUA8C4S2C*/t/agent.sock x",
        True,
    ),
    ("echo op://X/Admin/k 'unterminated", True),
    ("grep -rn Quote programs/", False),
    ("rg 'Admin' sdk/", False),
    ("echo op://Dev/feeds-api/credential", False),
    # Rule 2: op at any command position.
    ("op read op://x/y/z", True),
    ("op item list", True),
    ("/opt/homebrew/bin/op whoami", True),
    ("env FOO=1 op read x", True),
    ("env -u HOME op read x", True),
    ("sudo -u root op read x", True),
    ("ls && op vault list", True),
    ("ls | op inject", True),
    ("true; op signin", True),
    ("ls;(op read x)", True),
    ("true&&(op whoami)", True),
    ("(ls)|op read x", True),
    ("bash -c 'op read x'", True),
    ('zsh -lc "op read x"', True),
    ("sh -c 'ls; op read x'", True),
    ("bash -c \"true\nop read 'x'\"", True),
    ("eval op read x", True),
    ('echo "$(op read x)"', True),
    ("echo $(op read x)", True),
    ("echo `op read x`", True),
    ("xargs -n1 op read < refs", True),
    ("xargs --max-args 1 op read < refs", True),
    ("find . -name x -exec op read {} \\;", True),
    ("timeout 5 op whoami", True),
    ("time -p op whoami", True),
    ("watch 'op read x'", True),
    ("parallel op read ::: x", True),
    ("FOO=1 op run -- ls", True),
    ("env -S 'op read x'", True),
    ("$'op' read x", True),
    ("bash <<< 'op read x'", True),
    ("echo 'op read x' | sh", True),
    ("ls\nop read x", True),
    ("op \\\nread x", True),
    ("echo 'a\nb'; op read x", True),
    ("which op", False),
    ("command -v op", False),
    ("command -p -v op", False),
    ("echo op read", False),
    ("make collectors-up", False),
    ("git log --oneline", False),
    ("grep -rn 'op run' Makefile", False),
    ("git commit -m 'Subject\n\nop read is now blocked here.\n\nEnd'", False),
    ('git commit -m "Subject\n\nmake tui-mainnet now refuses agents"', False),
    ("cat script.sh | bash", False),
    # Rule 3: agent and session handles.
    ("SSH_AUTH_SOCK=/tmp/x git commit -S -m x", True),
    ("SSH_AUTH_SOCK+=/x git log", True),
    ("export SSH_AUTH_SOCK=/tmp/x", True),
    ("printf -v SSH_AUTH_SOCK %s /x", True),
    ("env OP_SESSION_my=abc ls", True),
    ("OP_SERVICE_ACCOUNT_TOKEN=x make collectors-up", True),
    ("ssh-add -l", True),
    ("ssh-keygen -Y sign -n file -f key x", True),
    ("ssh-keygen -Ysign -f key x", True),
    ("some-tool --session abc", True),
    ("some-tool --session=abc", True),
    ("ssh-keygen -l -f key.pub", False),
    ("ssh-keygen -Y verify -f allowed -n git", False),
    ("echo $SSH_AUTH_SOCK", False),
    ("grep -rn 'SSH_AUTH_SOCK=' docs/", False),
    ("claude --session-id abc", False),
    # Rule 4: the mainnet cockpit.
    ("make tui-mainnet", True),
    ("make -C tui tui-mainnet", True),
    ("TUI_KEYS_FILE=x make tui-mainnet", True),
    ("make teardown ARGS='--rpc-url https://api.mainnet-beta.solana.com'", True),
    ('make teardown ARGS="--rpc-url https://api.devnet.solana.com --yes"', True),
    ("make teardown ARGS='--rpc-url=https://rpc.example.com'", True),
    (
        "make teardown ARGS='--rpc-url http://localhost:8899@api.mainnet-beta.solana.com'",
        True,
    ),
    ("ARGS='--rpc-url https://api.mainnet-beta.solana.com' make teardown", True),
    ("export ARGS='--rpc-url https://x.io'; make teardown", True),
    ("make teardown ARGS='--rpc-url http://127.0.0.1:8899'", False),
    ("make teardown ARGS='--rpc-url http://localhost:8899'", False),
    ("make teardown", False),
    ("dropset-tui --cluster mainnet", True),
    ("./target/release/dropset-tui --cluster=mainnet-beta", True),
    ("dropset-tui --keys-file tui/private-keys.env", True),
    ("cargo run -p dropset-tui -- --cluster mainnet", True),
    ("cargo r -p dropset-tui -- --cluster mainnet", True),
    ("cargo run --manifest-path tui/Cargo.toml -- --cluster mainnet", True),
    ("cargo run -- --keys-file x", True),
    ("cargo run --package dropset-tui -- --keys-file x", True),
    ("cargo run -p dropset-tui --bin dropset-teardown -- --rpc-url https://x.io", True),
    ("dropset-teardown --rpc-url https://api.mainnet-beta.solana.com", True),
    ("cargo run -p dropset-tui -- --cluster localnet", False),
    ("cargo test -p dropset-tui", False),
    ("cargo build -p dropset-tui --release", False),
    ("make mainnet-demo", False),
    ("make tui", False),
    # Rule 5: Bash mutation of a reference file.
    ("echo x > tui/private-keys.env", True),
    ("echo x >> frontend/.env.local", True),
    ("echo x >& frontend/.env.local", True),
    ("true;>tui/private-keys.env", True),
    ("printf x >/Users/x/repos/dropset/infra/localnet/secrets.local.env", True),
    ("echo x > ../../../frontend/.env.local", True),
    ("rm tui/private-keys.env", True),
    ("mv tui/private-keys.env /tmp/x", True),
    ("cp /tmp/x frontend/.env.local", True),
    ("cp ../../../tui/private-keys.env tui/", True),
    ("cp -t tui /tmp/private-keys.env", True),
    ("ln -s /tmp/private-keys.env tui/", True),
    ("tee -a infra/localnet/secrets.local.env", True),
    ("sed -i '' s/a/b/ frontend/.env.local", True),
    ("perl -pi -e s/a/b/ frontend/.env.local", True),
    ("ln -sf /tmp/x tui/private-keys.env", True),
    ("echo x > ~/.config/1Password/ssh/agent.toml", True),
    ("echo x > $HOME/.config/1Password/ssh/agent.toml", True),
    ("cat tui/private-keys.env", False),
    ("cp tui/private-keys.env.example /tmp/x", False),
    ("cp frontend/.env.local /tmp/inspect", False),
    ("echo x > tui/private-keys.env.example", False),
    ("sed -n 1p frontend/.env.local", False),
    ("perl -Mstrict -e 1 frontend/.env.local", False),
    ("ls 2>&1", False),
    # Fails open on an unbalanced quote (the name checks above still ran).
    ("op read 'unterminated", False),
]
EDIT_CASES = [
    ("Edit", "tui/private-keys.env", True),
    ("Write", "/Users/x/repos/dropset/frontend/.env.local", True),
    ("MultiEdit", TEST_CWD + "/infra/localnet/secrets.local.env", True),
    ("Write", "../../../tui/private-keys.env", True),
    ("Edit", "~/.config/1Password/ssh/agent.toml", True),
    ("Edit", "tui/private-keys.env.example", False),
    ("Write", "frontend/.env.example", False),
    ("Edit", "tui/src/main.rs", False),
    ("Read", "tui/private-keys.env", False),
]


def _self_test():
    """Built-in cases, run with `--self-test` so it needs no piped stdin."""
    env = {VAULT_ENV: TEST_VAULT}
    failures = []
    for cmd, should_block in BASH_CASES:
        payload = {"tool_name": "Bash", "tool_input": {"command": cmd}}
        blocked = evaluate(payload, env, TEST_CWD)[0] == 2
        if blocked != should_block:
            failures.append(
                "  %-60r expected block=%s got block=%s" % (cmd, should_block, blocked)
            )
    for tool, path, should_block in EDIT_CASES:
        payload = {"tool_name": tool, "tool_input": {"file_path": path}}
        blocked = evaluate(payload, env, TEST_CWD)[0] == 2
        if blocked != should_block:
            failures.append(
                "  %s %-50r expected block=%s got block=%s"
                % (tool, path, should_block, blocked)
            )
    _, message = evaluate(
        {"tool_name": "Bash", "tool_input": {"command": "op read x"}}, env, TEST_CWD
    )
    for phrase in (
        "What tripped: a direct `op`",
        "every agent session and every\nsub-agent",
        "cannot be rotated",
        "Stop here",
    ):
        if phrase not in message:
            failures.append("  deny message lacks %r" % phrase)
    if failures:
        sys.stderr.write("self-test FAILED:\n" + "\n".join(failures) + "\n")
        return 1
    total = len(BASH_CASES) + len(EDIT_CASES)
    sys.stdout.write("self-test OK (%d cases)\n" % total)
    return 0


def main(argv):
    if "--self-test" in argv:
        return _self_test()
    try:
        payload = json.loads(sys.stdin.read())
    except Exception:
        # Fail open: a read or parse problem must never wedge the session.
        return 0
    code, message = evaluate(payload)
    if message:
        sys.stderr.write(message + "\n")
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
