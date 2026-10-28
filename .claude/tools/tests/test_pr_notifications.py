"""Stdlib ``unittest`` tests for the PR notification dismissal tool.

Run via the repo's ``make tools-tests`` (discovery adds ``.claude/tools`` as
the top-level dir so the bare ``import pr_notifications`` below resolves).
"""

import io
import json
import unittest
from unittest import mock

import pr_notifications
from pr_notifications import compact, dismiss, select, summary

URL = "https://api.github.com/repos/DASMAC-com/dropset/{}/{}"

# The inbox: two threads on a merged PR whose issue sits In Review (the bulk of
# the pile the board gate kept), one on a closed PR, one on an open PR, one on
# an old PR outside the listed set, one on an issue (not a PR) and one from
# another repository.
THREADS = [
    {"id": "m1", "subject": {"url": URL.format("pulls", 663)}},
    {"id": "m2", "subject": {"url": URL.format("pulls", 663)}},
    {"id": "c1", "subject": {"url": URL.format("pulls", 660)}},
    {"id": "o1", "subject": {"url": URL.format("pulls", 677)}},
    {"id": "x1", "subject": {"url": URL.format("pulls", 12)}},
    {"id": "i1", "subject": {"url": URL.format("issues", 663)}},
    {"id": "r1", "subject": {"url": "https://api.github.com/repos/a/b/pulls/663"}},
]
LISTED = [
    {"number": 663, "state": "MERGED"},
    {"number": 660, "state": "CLOSED"},
    {"number": 677, "state": "OPEN"},
]


class FakeGh:
    """Answers the paged list, the PR list, per-PR views and DELETEs; records
    every call. ``pages`` is the thread list split into pages."""

    def __init__(
        self,
        pages=None,
        listed=LISTED,
        views=None,
        list_fails=False,
        pr_list_fails=False,
        delete_fails=(),
    ):
        self.pages = [THREADS] if pages is None else pages
        self.listed = listed
        self.views = views or {}
        self.list_fails = list_fails
        self.pr_list_fails = pr_list_fails
        self.delete_fails = set(delete_fails)
        self.calls = []

    def __call__(self, args):
        self.calls.append(args)
        if args[0] == "api" and "notifications?" in args[1]:
            if self.list_fails:
                return 1, "", "HTTP 401: Bad credentials"
            page = int(args[1].rsplit("page=", 1)[1])
            body = self.pages[page - 1] if page <= len(self.pages) else []
            return 0, json.dumps(body), ""
        if args[:2] == ["pr", "list"]:
            if self.pr_list_fails:
                return 1, "", "HTTP 502"
            return 0, json.dumps(self.listed), ""
        if args[:2] == ["pr", "view"]:
            num = int(args[2])
            if num in self.views:
                return 0, json.dumps({"state": self.views[num]}), ""
            return 1, "", "no pull requests found"
        if args[:2] == ["api", "-X"]:
            tid = args[-1].rsplit("/", 1)[1]
            return (1, "", "HTTP 500") if tid in self.delete_fails else (0, "", "")
        raise AssertionError(f"unexpected gh call {args}")

    def deletes(self):
        return [a[-1].rsplit("/", 1)[1] for a in self.calls if a[:2] == ["api", "-X"]]


class SettledTests(unittest.TestCase):
    def test_merged_and_closed_are_selected_whatever_the_issue_state(self):
        gh = FakeGh(views={12: "MERGED"})
        rc, out = dismiss(gh, None, apply=True)
        self.assertEqual(rc, 0)
        # Only PR threads on this repository count, and the open PR's never.
        self.assertEqual(out["threads"], 5)
        self.assertEqual(sorted(gh.deletes()), ["c1", "m1", "m2", "x1"])
        self.assertEqual(out["dismissed"], 4)
        self.assertEqual(out["unresolved"], [])

    def test_an_open_pr_is_never_selected(self):
        gh = FakeGh()
        _, out = dismiss(gh, None, apply=True)
        self.assertNotIn("o1", gh.deletes())
        self.assertNotIn(677, {t["pr"] for t in out["selected"]})

    def test_a_pr_with_no_known_state_is_left_and_reported(self):
        gh = FakeGh()  # #12 is outside the list and its view fails
        _, out = dismiss(gh, None, apply=True)
        self.assertEqual(out["unresolved"], [12])
        self.assertNotIn("x1", gh.deletes())
        self.assertIn("1 PR(s) unresolved", summary(out))

    def test_lookups_outside_the_list_are_capped(self):
        many = [
            {"id": f"t{n}", "subject": {"url": URL.format("pulls", n)}}
            for n in range(1, 31)
        ]
        gh = FakeGh(pages=[many], listed=[])
        dismiss(gh, None, apply=False)
        views = [a for a in gh.calls if a[:2] == ["pr", "view"]]
        self.assertEqual(len(views), pr_notifications.MAX_LOOKUPS)

    def test_a_failed_pr_list_falls_back_to_lookups_and_says_so(self):
        gh = FakeGh(pr_list_fails=True, views={663: "MERGED"})
        _, out = dismiss(gh, None, apply=False)
        self.assertIn("PR list failed", out["notice"])
        self.assertEqual({t["pr"] for t in out["selected"]}, {663})


class PrModeTests(unittest.TestCase):
    def test_named_prs_are_selected_whatever_their_state(self):
        gh = FakeGh()
        _, out = dismiss(gh, {663, 677}, apply=True)
        self.assertEqual(sorted(gh.deletes()), ["m1", "m2", "o1"])
        # No state is read: the close path has already refused an open PR.
        self.assertFalse(
            any(a[:2] in (["pr", "list"], ["pr", "view"]) for a in gh.calls)
        )
        self.assertEqual(out["mode"], "pr")


class ApplyTests(unittest.TestCase):
    def test_a_dry_run_deletes_nothing(self):
        gh = FakeGh()
        rc, out = dismiss(gh, None, apply=False)
        self.assertEqual(rc, 0)
        self.assertEqual(gh.deletes(), [])
        self.assertTrue(out["dry_run"])
        self.assertEqual(
            summary(out), "would dismiss 3 notification(s); 1 PR(s) unresolved, left"
        )

    def test_one_delete_per_thread_never_a_mark_all(self):
        gh = FakeGh()
        dismiss(gh, {663}, apply=True)
        writes = [a for a in gh.calls if a[:2] == ["api", "-X"]]
        self.assertEqual(
            writes,
            [
                ["api", "-X", "DELETE", "/notifications/threads/m1"],
                ["api", "-X", "DELETE", "/notifications/threads/m2"],
            ],
        )

    def test_a_failed_delete_is_counted(self):
        gh = FakeGh(delete_fails=["m2"])
        _, out = dismiss(gh, {663}, apply=True)
        self.assertEqual(out["dismissed"], 1)
        self.assertEqual(out["failed"], ["m2"])
        self.assertEqual(summary(out), "dismissed 1 notification(s), 1 failed")

    def test_an_unreadable_list_exits_one_and_dismisses_nothing(self):
        gh = FakeGh(list_fails=True)
        rc, out = dismiss(gh, None, apply=True)
        self.assertEqual(rc, 1)
        self.assertIn("Bad credentials", out["notice"])
        self.assertEqual(gh.deletes(), [])


class PagingTests(unittest.TestCase):
    def test_pages_until_a_short_page(self):
        per = pr_notifications.PER_PAGE
        full = [
            {"id": f"t{i}", "subject": {"url": URL.format("pulls", 663)}}
            for i in range(per)
        ]
        gh = FakeGh(pages=[full, THREADS[:1]])
        _, out = dismiss(gh, {663}, apply=False)
        pages = [a for a in gh.calls if a[0] == "api" and "notifications?" in a[1]]
        self.assertEqual(len(pages), 2)
        self.assertIn("all=true", pages[0][1])
        self.assertIn("/repos/DASMAC-com/dropset/notifications", pages[0][1])
        self.assertEqual(len(out["selected"]), per + 1)

    def test_paging_stops_at_the_cap(self):
        per = pr_notifications.PER_PAGE
        full = [
            {"id": f"t{i}", "subject": {"url": URL.format("pulls", 663)}}
            for i in range(per)
        ]
        gh = FakeGh(pages=[full] * (pr_notifications.MAX_PAGES + 3))
        dismiss(gh, {663}, apply=False)
        pages = [a for a in gh.calls if a[0] == "api" and "notifications?" in a[1]]
        self.assertEqual(len(pages), pr_notifications.MAX_PAGES)


class MalformedListTests(unittest.TestCase):
    def test_an_object_page_is_unreadable_not_zero_threads(self):
        # gh exits 0 with an error body: never read as an empty inbox, and
        # never an AttributeError escaping to an in-process caller.
        gh = FakeGh(pages=[{"message": "Bad credentials"}])
        rc, out = dismiss(gh, None, apply=True)
        self.assertEqual(rc, 1)
        self.assertIn("not a thread list", out["notice"])
        self.assertEqual(gh.deletes(), [])

    def test_malformed_items_are_skipped(self):
        page = ["junk", {"id": "s1", "subject": "x"}, {"id": "s2"}, THREADS[0]]
        gh = FakeGh(pages=[page])
        _, out = dismiss(gh, {663}, apply=False)
        self.assertEqual(out["threads"], 1)
        self.assertEqual([t["id"] for t in out["selected"]], ["m1"])


class CliTests(unittest.TestCase):
    def parse(self, *argv):
        return pr_notifications.parse_args(["prog", *argv])

    def test_pr_is_repeatable_and_dry_run_is_the_default(self):
        args = self.parse("dismiss", "--pr", "663", "--pr", "664")
        self.assertEqual(args.pr, [663, 664])
        self.assertFalse(args.settled)
        self.assertFalse(args.apply)

    def test_exactly_one_selector_is_required(self):
        for argv in (["dismiss"], ["dismiss", "--pr", "1", "--settled"]):
            with self.assertRaises(SystemExit), mock.patch("sys.stderr"):
                self.parse(*argv)

    def test_main_passes_mode_and_apply_through_and_prints_compact(self):
        seen = []

        def fake_dismiss(gh, prs, apply):
            seen.append((prs, apply))
            return 0, {
                "mode": "settled", "dry_run": not apply, "threads": 0,
                "selected": [], "dismissed": 0, "failed": [],
                "unresolved": [], "notice": None,
            }  # fmt: skip

        out = io.StringIO()
        with mock.patch.object(pr_notifications, "dismiss", fake_dismiss):
            for argv in (["dismiss", "--settled", "--apply"], ["dismiss", "--pr", "7"]):
                with (
                    mock.patch("sys.argv", ["prog", *argv]),
                    mock.patch("sys.stdout", out),
                ):
                    self.assertEqual(pr_notifications.main(), 0)
        self.assertEqual(seen, [(None, True), ({7}, False)])
        self.assertIn('"summary": "would dismiss 0 notification(s)"', out.getvalue())


class OutputTests(unittest.TestCase):
    def test_select_is_pure(self):
        threads = [("a", 1), ("b", 2), ("c", 3)]
        states = {1: "MERGED", 2: "OPEN"}
        selected, unresolved = select(threads, states, None)
        self.assertEqual([t["id"] for t in selected], ["a"])
        self.assertEqual(unresolved, [3])

    def test_compact_counts_instead_of_listing(self):
        _, out = dismiss(FakeGh(), {663}, apply=False)
        short = compact(out)
        self.assertEqual(short["selected"], 2)
        self.assertEqual(short["prs"], [663])
        self.assertEqual(short["summary"], "would dismiss 2 notification(s)")


if __name__ == "__main__":
    unittest.main()
