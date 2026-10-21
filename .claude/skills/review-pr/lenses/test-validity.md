# Lens: test validity

<!-- default-cap: ≤ 4 turns / ≤ 6 tool calls -->

**Your cap: `{{cap}}`, hard stop.**

## Dimension

You are the reduced tier's one lens for a **test-only** diff
(`#[cfg(test)]`, `tests/`, no production change). **Do the tests
assert the right thing?** A test that passes whatever the code does, a
fixture that no longer exercises the path it names, or an assertion on
the wrong value is a finding; an assertion that merely "could be
tighter" is not (see the standing suppressions).
