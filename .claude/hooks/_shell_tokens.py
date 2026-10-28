# cspell:word backslashreplace
# cspell:word caffeinate
# cspell:word commenters
# cspell:word execdir
# cspell:word okdir
# cspell:word procs
# cspell:word rcfile
# cspell:word sshlogin
"""The guard hooks' shared, quote-aware shell tokenizer.

Not a guard: the leading underscore keeps it out of ``hook_wiring.py``'s
guard list, and it wires to no event. A guard imports it from its own
directory (the harness runs a hook as a script, so ``sys.path[0]`` is
``.claude/hooks/``) and must keep its **fail-open** contract at that
boundary: a failed import (missing, or raising at import) disables the
guard's parse-driven checks, never the session.

The parse is **best-effort**, like every guard that reads bash: it models
the shapes an agent actually types, not the whole grammar. It was extracted
from the private-key guard after four adversarial review rounds, each of
which found a real quoting misread, so a new guard should start here rather
than re-derive bash quoting under review.

**Pipeline.** ``raw_pass`` makes one quote-aware pass over the raw text: it
decodes each ``$'…'`` outside quotes, drops comments, and collects the
command substitutions the shell would run, so a backtick or ``$(`` inside
single quotes (a PR body quoting a command) is literal while one inside
double quotes is live. Where it meets something it does not model — an
unclosed quote, a heredoc, a ``case`` inside ``$(`` — it says so, and the
caller decides how to fail. ``tokenize`` lexes the rewrite once, with an
unquoted newline as a command separator — so a quoted string spanning lines
stays one word — and splits runs of glued operators (``;(``, ``&&(``,
``;>``) into their parts. Only when the whole command's quotes do not
balance does it fall back to line by line, and a line that still does not
lex is split into plain words, never skipped. ``segments`` splits tokens
into commands at control operators; ``unwrap`` strips one command's
redirects, keywords, assignments and wrappers down to the argv that runs,
plus the nested command strings a shell would run on its behalf.

A heredoc body is scanned as shell text even under a quoted delimiter
(``<<'EOF'``, where bash keeps it literal): ``uncertain`` is set, and the
cost is a false block, never a missed command. The shell rules forbid
heredocs in agent commands anyway.

``.claude/tools/tests/test_shell_tokens.py`` pins the tokenizer directly;
the private-key guard's case table and the git-grep guard's self-test
pin it end to end through the guards that import it.
"""

import os
import re
import shlex

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
# Shell options that take the following word as their value, so it is not
# the script operand (`bash -o pipefail -c '…'`).
SHELL_VALUE_FLAGS = frozenset({"-o", "+o", "-O", "+O", "--rcfile", "--init-file"})

ASSIGN_RE = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)\+?=")
# `\Z`, never `$`: `$` also matches before a trailing newline, and a quoted
# `read 'x<newline>'` operand must not read as the name `x`.
NAME_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*\Z")
# Builtins whose operands are assignments (`export X=1`) or, for the last two,
# bare variable names they set (`printf -v X …`, `read X`).
DECLARERS = frozenset({"export", "declare", "typeset", "local", "readonly"})
NAME_SETTERS = frozenset({"printf", "read"})

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
    "coproc": frozenset(),
}

PLAIN_WORD_RE = re.compile(r"[();<>|&]+|[^();<>|&]+")
# Characters after which a `#` starts a comment rather than sitting mid-word.
COMMENT_START_AFTER = " \t\n;&|()"


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


def _ansi_c_word(content):
    """A `$'…'` body, decoded, re-spelled as a double-quoted word.

    `$'it\\'s'` escapes its quote with a backslash, which a plain single quote
    cannot, and `$'\\x6fp'` spells `op` in hex: decode the escapes so the lexer
    and the checks see the word the shell would.
    """
    try:
        word = content.encode("latin-1", "backslashreplace").decode("unicode_escape")
    except (UnicodeError, ValueError):
        word = re.sub(r"\\(.)", r"\1", content)
    return '"' + word.replace("\\", "\\\\").replace('"', '\\"') + '"'


def _ansi_c_end(text, i):
    """Index of the quote closing a `$'…'` body that starts at ``i``, or -1."""
    n = len(text)
    while i < n:
        if text[i] == "\\":
            i += 2
            continue
        if text[i] == "'":
            return i
        i += 1
    return -1


def _paren_end(text, i):
    """Index of the `)` closing a `$(` whose body starts at ``i``.

    Quote-aware, so a `)` inside a quoted string or after a backslash does
    not end the body early. Returns ``len(text)`` when nothing closes it.
    """
    depth = 1
    quote = None
    n = len(text)
    while i < n:
        c = text[i]
        if quote == "'":
            if c == "'":
                quote = None
        elif c == "\\":
            i += 1
        elif c == '"':
            quote = None if quote == '"' else '"'
        elif quote is None and c == "'":
            quote = "'"
        elif quote is None and c == "(":
            depth += 1
        elif quote is None and c == ")":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return n


def raw_pass(text):
    """One quote-aware pass over the raw command text.

    Returns ``(rewritten, bodies, uncertain)``. ``rewritten`` has each
    `$'…'` outside quotes re-spelled for the lexer and each comment dropped;
    ``bodies`` are the command substitutions the shell would run (a backtick
    or `$(` inside single quotes is literal, inside double quotes or bare it
    runs); ``uncertain`` is True when the text holds something this pass does
    not model — an unclosed quote or substitution, a heredoc, a `case` inside
    `$(` — and the caller must fall back to a cruder fail-closed scan.
    """
    out = []
    bodies = []
    quote = None
    uncertain = False
    # Whether the next character begins a word, the only place a `#` starts
    # a comment. Tracked explicitly: the previous raw character cannot say,
    # since `$(true)#`, `\;#` and `a\ #` are all mid-word to the shell.
    word_start = True
    i = 0
    n = len(text)
    while i < n:
        c = text[i]
        if c == "#" and quote is None and word_start:
            end = text.find("\n", i)
            i = n if end == -1 else end
            continue
        word_start = False
        if quote == "'":
            if c == "'":
                quote = None
            out.append(c)
            i += 1
            continue
        if c == "\\":
            out.append(text[i : i + 2])
            i += 2
            continue
        if text.startswith("$$", i):
            # The PID parameter: the second `$` is spent, so a `'` after it
            # is a plain quote, not ANSI-C (`$$'\'` is not `$'\''`).
            out.append("$$")
            i += 2
            continue
        if quote is None:
            if text.startswith("$'", i):
                end = _ansi_c_end(text, i + 2)
                if end == -1:
                    uncertain = True
                    out.append(text[i:])
                    break
                out.append(_ansi_c_word(text[i + 2 : end]))
                i = end + 1
                continue
            if c == "'":
                quote = "'"
                out.append(c)
                i += 1
                continue
            if text.startswith("<<", i) and not text.startswith("<<<", i):
                uncertain = True
        if c == '"':
            quote = None if quote == '"' else '"'
        elif c == "`":
            end = text.find("`", i + 1)
            if end == -1:
                uncertain = True
                end = n
            bodies.append(text[i + 1 : end])
            out.append(text[i : end + 1])
            i = end + 1
            continue
        elif text.startswith("$(", i):
            end = _paren_end(text, i + 2)
            body = text[i + 2 : end]
            if end >= n or re.search(r"\bcase\b", body):
                # A `case` pattern's bare `)` defeats the depth count; take
                # everything after the `$(` rather than a body cut short.
                uncertain = True
                body = text[i + 2 :]
            bodies.append(body)
            out.append(text[i : end + 1])
            i = end + 1
            continue
        out.append(c)
        i += 1
        # Only an unescaped, unquoted blank or operator character starts a
        # word; every other path through the loop leaves this False.
        word_start = quote is None and c in COMMENT_START_AFTER
    if quote is not None:
        uncertain = True
    return "".join(out), bodies, uncertain


def _plain_words(line):
    """A line that does not lex, split on whitespace and operator runs.

    Scanning it word by word, rather than skipping it, keeps a command word
    visible on a line whose quotes are odd.
    """
    words = []
    for word in line.split():
        words.extend(PLAIN_WORD_RE.findall(word))
    return words


def tokenize(text):
    """Quote-aware tokens with an unquoted newline as its own `\\n` token.

    ``text`` is `raw_pass`'s rewrite. Falls back to line-by-line only when
    the whole command's quotes do not balance; a line that still does not lex
    is split into plain words.
    """
    # Locale quoting (`$"op"`) quotes exactly like a plain double quote for the
    # words a guard reads.
    text = re.sub(r"\$(?=\")", "", text)
    try:
        raw = _lex(text)
    except ValueError:
        raw = []
        for line in text.split("\n"):
            try:
                raw.extend(_lex(line))
            except ValueError:
                raw.extend(_plain_words(line))
            raw.append("\n")
    tokens = []
    for tok in raw:
        tokens.extend(_split_operators(tok))
    return tokens


def segments(tokens):
    """Split tokens into commands at control operators.

    Returns ``(segment, separator)`` pairs, the separator being the operator
    that ended the previous command (None for the first), so a shell reading
    a pipe can find what is piped into it.
    """
    parts = [([], None)]
    for tok in tokens:
        if tok in CONTROL:
            parts.append(([], tok))
        else:
            parts[-1][0].append(tok)
    return [(s, sep) for s, sep in parts if s]


def unwrap(seg):
    """Strip redirects, keywords, assignments and wrappers.

    Returns ``(argv, nested, assigns, reads_stdin)``: ``nested`` holds command
    strings to scan recursively (a `-c` payload, an `env -S` string, `eval`'s
    or `watch`'s words, `find -exec`'s command, a here-string fed to a shell);
    ``assigns`` every variable assignment the command makes, a name set by
    `printf -v` / `read` spelled ``NAME=``; ``reads_stdin`` is True for a
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
        j = 1
        while j < len(argv):
            tok = argv[j]
            if tok.startswith("-") and not tok.startswith("--") and "c" in tok[1:]:
                payload = argv[j + 1] if j + 1 < len(argv) else None
                break
            if tok in SHELL_VALUE_FLAGS:
                j += 2
                continue
            if not tok.startswith(("-", "+")):
                operand = True
                break
            j += 1
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
        assigns.extend(a + "=" for a in argv[1:] if NAME_RE.match(a))
    return argv, nested, assigns, reads_stdin
