#!/usr/bin/env python3
# cspell:word caffeinate
# cspell:word commenters
# cspell:word execdir
# cspell:word gsed
# cspell:word okdir
"""PreToolUse guard: keep every agent session away from the private keys.

The mainnet private keys live in a 1Password vault and sign through the
1Password SSH agent. No agent session or sub-agent ever needs them, and a
private key cannot be rotated once it leaks: the address it controls is the
identity. So this guard denies, with **no escape marker**, every tool call
that reaches toward them:

1. **Names.** Any Bash command naming the private-keys vault (its name or id,
   read from ``DS_OP_PRIVATE_KEYS_VAULT`` in the session environment, comma
   separated for both), an ``op://`` reference to one of the key items
   (``eBook``, ``Admin``, ``Lead``, ``Quote`` and their signing duplicates —
   matched on the item's first word), or the 1Password agent socket. The
   committed script carries no real vault name. The bare titles are matched
   only inside an ``op://`` reference: ``Admin`` and ``Quote`` are ordinary
   words in this codebase, and a guard that blocks ``grep Quote`` gets turned
   off.
2. **``op`` at any command position**, including after ``env``, ``sudo``,
   ``xargs``, ``find -exec``, a control operator, inside a ``-c`` payload,
   ``eval``, or a command substitution — the shapes a ``Bash(op:*)`` prefix
   deny rule cannot see. The make targets carry the ``op run`` the
   collectors and the keyed frontend need, so an agent never types ``op``.
3. **The agent and session handles.** Any ``SSH_AUTH_SOCK``,
   ``OP_SESSION_*`` or ``OP_SERVICE_ACCOUNT_TOKEN`` assignment, ``ssh-add``,
   ``ssh-keygen -Y sign``, and the ``--session`` flag.
4. **The mainnet cockpit.** ``make tui-mainnet``; ``make teardown`` (or
   ``dropset-teardown``) with a non-localnet ``--rpc-url``; ``dropset-tui``
   or ``cargo run -p dropset-tui`` with ``--cluster mainnet`` or
   ``--keys-file``. ``make mainnet-demo`` (watch-only, ephemeral key) stays
   allowed here; the binary itself refuses an agent shell on mainnet.
5. **The four reference files** — ``tui/private-keys.env``,
   ``infra/localnet/secrets.local.env``, ``frontend/.env.local`` and
   ``~/.config/1Password/ssh/agent.toml`` — may not be mutated, through
   ``Edit`` / ``Write`` / ``MultiEdit`` or through Bash (a redirection, or a
   mutating command such as ``rm``, ``mv``, ``tee`` or ``sed -i`` naming
   one). A path is matched by suffix after ``~`` expansion and resolution
   against the checkout, so the base copy, every worktree's copy, and a
   ``../../../`` spelling all match. Reading them is allowed: they hold
   ``op://`` references, never key material.

The ``permissions.deny`` rules in the user-local settings are the other
layer (``docs/conventions/local-integrations.md`` → "The private-key guard
hook"): they hold under auto mode with no script running. This hook runs
first, so its message is what the agent reads, and it covers the shapes a
prefix rule cannot express.

**What neither layer can stop**: a script whose contents a prefix rule and
this tokenizer never see (``make`` running a recipe, ``python3 x.py``), an
MCP tool that shells out, and a sub-agent or session without the wiring. The
layer that holds there is the 1Password SSH agent's own per-key prompt: a
signing request from any process names the key and the requesting process,
and the operator refuses it.

Reads the PreToolUse payload as JSON on stdin; exits 2 with the reason on
stderr to block. Fails **open** on an unparseable payload or a line whose
quotes do not balance (the shell would reject that line too), though the
name checks run on the raw text first and need no parse. Run the built-in
cases with ``--self-test``; ``.claude/tools/tests/test_no_private_key_access.py``
pins them under ``make tools-tests``.
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

# Assignments whose very presence hands a process a key-signing or vault
# session handle.
HANDLE_ASSIGN_RE = re.compile(
    r"^(SSH_AUTH_SOCK|OP_SESSION_[A-Za-z0-9_]*|OP_SERVICE_ACCOUNT_TOKEN)="
)
ASSIGN_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")

# The reference files, as checkout-relative suffixes; the agent config is a
# home path.
REFERENCE_SUFFIXES = (
    "tui/private-keys.env",
    "infra/localnet/secrets.local.env",
    "frontend/.env.local",
)
AGENT_CONFIG = "~/.config/1Password/ssh/agent.toml"

EDIT_TOOLS = ("Edit", "Write", "MultiEdit")

# Tokens after which the next word is a command. Any punctuation token ending
# in `(` (`(`, `$(`'s `(`, `<(`) also opens one.
CONTROL = frozenset({"|", "||", "&&", ";", "&", "|&", ";;", "(", ")", "{", "}", "!"})
KEYWORDS = frozenset(
    {"then", "do", "else", "elif", "if", "while", "until", "time", "!", "{"}
)
REDIRECTS = frozenset({">", ">>", ">|", "&>", "&>>", "<>"})

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
    "command": frozenset(),
    "nice": frozenset({"-n"}),
    "caffeinate": frozenset({"-w", "-t"}),
    "stdbuf": frozenset({"-i", "-o", "-e"}),
    "time": frozenset(),
    "timeout": frozenset({"-s", "--signal", "-k", "--kill-after"}),
    "xargs": frozenset({"-I", "-L", "-n", "-P", "-s", "-d", "-E", "-a"}),
    "watch": frozenset({"-n", "-d"}),
}

# Commands that mutate every file they name, and those that mutate only their
# last operand (copying *from* a reference file is a read).
MUTATE_ALL = frozenset(
    {"rm", "mv", "tee", "truncate", "ln", "touch", "chmod", "chown", "shred", "unlink"}
)
MUTATE_LAST = frozenset({"cp", "rsync", "install", "ditto"})
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
    if "1password" in low and "agent.sock" in low:
        return "the command names the 1Password SSH agent socket"
    return None


def _tokens(line):
    lex = shlex.shlex(line, posix=True, punctuation_chars=True)
    lex.whitespace_split = True
    lex.commenters = ""
    return list(lex)


def _is_control(tok):
    return tok in CONTROL or (tok.endswith("(") and set(tok) <= set("<>$("))


def _segments(tokens):
    """Split tokens into commands at control operators."""
    segments_out = [[]]
    for tok in tokens:
        if _is_control(tok):
            segments_out.append([])
        else:
            segments_out[-1].append(tok)
    return [s for s in segments_out if s]


def _resolve(path, cwd):
    path = os.path.expanduser(path)
    if not os.path.isabs(path):
        path = os.path.join(cwd or os.getcwd(), path)
    return os.path.normpath(path)


def _reference_file(path, cwd):
    """The reference file a path names, or None."""
    if not isinstance(path, str) or not path:
        return None
    resolved = _resolve(path, cwd)
    low = resolved.lower()
    for suffix in REFERENCE_SUFFIXES:
        if low == suffix or low.endswith("/" + suffix):
            return suffix
    if low == os.path.normpath(os.path.expanduser(AGENT_CONFIG)).lower():
        return AGENT_CONFIG
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
    if url.lower() in LOCAL_MONIKERS:
        return True
    host = re.sub(r"^[a-z]+://", "", url.lower())
    if host.startswith("["):
        host = host.split("]", 1)[0] + "]"
    else:
        host = host.split("/", 1)[0].rsplit(":", 1)[0]
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


def _make_reason(args):
    targets = []
    make_args = []
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
            try:
                make_args = shlex.split(tok.split("=", 1)[1])
            except ValueError:
                make_args = tok.split("=", 1)[1].split()
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
    if "run" not in args:
        return None
    package = None
    binary = None
    for i, tok in enumerate(args):
        nxt = args[i + 1] if i + 1 < len(args) else ""
        if tok in ("-p", "--package"):
            package = nxt
        elif tok.startswith("--package="):
            package = tok.split("=", 1)[1]
        elif tok.startswith("-p") and len(tok) > 2 and not tok.startswith("--"):
            package = tok[2:]
        elif tok == "--bin":
            binary = nxt
        elif tok.startswith("--bin="):
            binary = tok.split("=", 1)[1]
    if binary == "dropset-teardown":
        return _teardown_reason(args)
    if package == "dropset-tui" or binary == "dropset-tui":
        return _cockpit_reason(args)
    return None


def _in_place_flag(arg):
    """`-i`, `--in-place`, or a short-flag cluster carrying `i` (`perl -pi`)."""
    if arg.startswith("--"):
        return arg.startswith("--in-place")
    return arg.startswith("-") and "i" in arg[1:]


def _mutation_reason(name, args, cwd):
    """Rule 5 through a mutating command's operands."""
    operands = [a for a in args if not a.startswith("-")]
    if name in MUTATE_ALL:
        candidates = operands
    elif name in MUTATE_LAST:
        candidates = operands[-1:]
    elif name in IN_PLACE and any(_in_place_flag(a) for a in args):
        candidates = operands
    elif name == "dd":
        candidates = [a[3:] for a in args if a.startswith("of=")]
    else:
        return None
    for path in candidates:
        ref = _reference_file(path, cwd)
        if ref:
            return "`%s` would modify the reference file %s" % (name, ref)
    return None


def _unwrap(seg):
    """Strip keywords, assignments and wrappers; return (argv, nested).

    `nested` holds command strings to scan recursively (a `-c` payload, an
    `env -S` string, `eval`'s words). argv is empty when there is no command.
    """
    i = 0
    while i < len(seg) and (seg[i] in KEYWORDS or ASSIGN_RE.match(seg[i])):
        i += 1
    argv = seg[i:]
    nested = []
    while argv:
        name = os.path.basename(argv[0])
        if name not in WRAPPERS:
            break
        if name == "command" and any(a in ("-v", "-V") for a in argv[1:2]):
            return [], nested
        value_flags = WRAPPERS[name]
        j = 1
        positional_skipped = False
        while j < len(argv):
            tok = argv[j]
            if name == "env" and (tok == "-S" or tok == "--split-string"):
                if j + 1 < len(argv):
                    nested.append(argv[j + 1])
                return [], nested
            if tok == "--":
                j += 1
                break
            if tok in value_flags:
                j += 2
                continue
            if tok.startswith("-"):
                j += 1
                continue
            if ASSIGN_RE.match(tok) and name in ("env", "sudo"):
                j += 1
                continue
            if name == "timeout" and not positional_skipped:
                positional_skipped = True
                j += 1
                continue
            break
        argv = argv[j:]
    if not argv:
        return [], nested
    name = os.path.basename(argv[0])
    if name in SHELLS:
        for j, tok in enumerate(argv[1:], start=1):
            if (
                tok.startswith("-")
                and not tok.startswith("--")
                and "c" in tok[1:]
                and j + 1 < len(argv)
            ):
                nested.append(argv[j + 1])
                break
    elif name == "eval":
        nested.append(" ".join(argv[1:]))
    elif name == "find":
        for j, tok in enumerate(argv):
            if tok in ("-exec", "-execdir", "-ok", "-okdir"):
                nested.append(shlex.join(argv[j + 1 :]))
    return argv, nested


def _command_reason(argv, cwd):
    name = os.path.basename(argv[0])
    args = argv[1:]
    if name == "op":
        return "a direct `op` (1Password CLI) invocation"
    if name == "ssh-add":
        return "`ssh-add`, which loads or lists signing keys in an SSH agent"
    if name == "ssh-keygen":
        for i, tok in enumerate(args):
            if tok == "-Ysign" or (
                tok == "-Y" and i + 1 < len(args) and args[i + 1] == "sign"
            ):
                return "`ssh-keygen -Y sign`, a signing request to the key agent"
    if name == "make" or name == "gmake":
        return _make_reason(args)
    if name == "dropset-tui":
        return _cockpit_reason(args)
    if name == "dropset-teardown":
        return _teardown_reason(args)
    if name == "cargo":
        return _cargo_reason(args)
    return _mutation_reason(name, args, cwd)


def _token_reason(tokens, cwd):
    """Checks that look at every token, whatever its position."""
    for i, tok in enumerate(tokens):
        match = HANDLE_ASSIGN_RE.match(tok)
        if match:
            return "an assignment to %s" % match.group(1)
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
    for line in command.split("\n"):
        try:
            tokens = _tokens(line)
        except ValueError:
            continue
        reason = _token_reason(tokens, cwd)
        if reason:
            return reason
        for tok in tokens:
            inner = None
            if "$(" in tok:
                inner = tok.split("$(", 1)[1]
            elif "`" in tok:
                inner = tok.replace("`", " ; ")
            if inner is not None and inner != line:
                reason = _scan(inner, cwd, depth + 1)
                if reason:
                    return reason
        for seg in _segments(tokens):
            argv, nested = _unwrap(seg)
            for payload in nested:
                reason = _scan(payload, cwd, depth + 1)
                if reason:
                    return reason
            if argv:
                reason = _command_reason(argv, cwd)
                if reason:
                    return reason
    return None


def evaluate(payload, env=None, cwd=None):
    """Return (exit_code, message). exit_code 2 blocks; 0 allows."""
    env = os.environ if env is None else env
    if not isinstance(payload, dict):
        return 0, ""
    tool = payload.get("tool_name")
    tool_input = payload.get("tool_input") or {}
    if not isinstance(tool_input, dict):
        return 0, ""
    cwd = cwd or payload.get("cwd") or env.get("CLAUDE_PROJECT_DIR") or os.getcwd()
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
    ("bash -c 'op read x'", True),
    ('zsh -lc "op read x"', True),
    ("sh -c 'ls; op read x'", True),
    ("eval op read x", True),
    ('echo "$(op read x)"', True),
    ("echo $(op read x)", True),
    ("echo `op read x`", True),
    ("xargs -n1 op read < refs", True),
    ("find . -name x -exec op read {} \\;", True),
    ("timeout 5 op whoami", True),
    ("FOO=1 op run -- ls", True),
    ("env -S 'op read x'", True),
    ("ls\nop read x", True),
    ("which op", False),
    ("command -v op", False),
    ("echo op read", False),
    ("make collectors-up", False),
    ("git log --oneline", False),
    ("grep -rn 'op run' Makefile", False),
    # Rule 3: agent and session handles.
    ("SSH_AUTH_SOCK=/tmp/x git commit -S -m x", True),
    ("export SSH_AUTH_SOCK=/tmp/x", True),
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
    ("claude --session-id abc", False),
    # Rule 4: the mainnet cockpit.
    ("make tui-mainnet", True),
    ("make -C tui tui-mainnet", True),
    ("TUI_KEYS_FILE=x make tui-mainnet", True),
    ("make teardown ARGS='--rpc-url https://api.mainnet-beta.solana.com'", True),
    ('make teardown ARGS="--rpc-url https://api.devnet.solana.com --yes"', True),
    ("make teardown ARGS='--rpc-url=https://rpc.example.com'", True),
    ("make teardown ARGS='--rpc-url http://127.0.0.1:8899'", False),
    ("make teardown ARGS='--rpc-url http://localhost:8899'", False),
    ("make teardown", False),
    ("dropset-tui --cluster mainnet", True),
    ("./target/release/dropset-tui --cluster=mainnet-beta", True),
    ("dropset-tui --keys-file tui/private-keys.env", True),
    ("cargo run -p dropset-tui -- --cluster mainnet", True),
    ("cargo run --package dropset-tui -- --keys-file x", True),
    ("cargo run -p dropset-tui --bin dropset-teardown -- --rpc-url https://x.io", True),
    ("dropset-teardown --rpc-url https://api.mainnet-beta.solana.com", True),
    ("cargo run -p dropset-tui -- --cluster localnet", False),
    ("cargo test -p dropset-tui", False),
    ("make mainnet-demo", False),
    ("make tui", False),
    # Rule 5: Bash mutation of a reference file.
    ("echo x > tui/private-keys.env", True),
    ("echo x >> frontend/.env.local", True),
    ("printf x >/Users/x/repos/dropset/infra/localnet/secrets.local.env", True),
    ("echo x > ../../../frontend/.env.local", True),
    ("rm tui/private-keys.env", True),
    ("mv tui/private-keys.env /tmp/x", True),
    ("cp /tmp/x frontend/.env.local", True),
    ("tee -a infra/localnet/secrets.local.env", True),
    ("sed -i '' s/a/b/ frontend/.env.local", True),
    ("ln -sf /tmp/x tui/private-keys.env", True),
    ("echo x > ~/.config/1Password/ssh/agent.toml", True),
    ("cat tui/private-keys.env", False),
    ("cp tui/private-keys.env.example /tmp/x", False),
    ("cp frontend/.env.local /tmp/inspect", False),
    ("echo x > tui/private-keys.env.example", False),
    ("sed -n 1p frontend/.env.local", False),
    # Fails open on an unbalanced quote (the name checks still ran).
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
    for phrase in ("What tripped: a direct `op`", "cannot be rotated", "Stop here"):
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
