//! Every parked source exists twice — once as a `dropset_feeds::PARKED_SOURCES`
//! entry, once as a deployment that does not start it — and this pins the two
//! together.
//!
//! **Why that needs a test.** The two say the same thing to different readers
//! and neither can derive the other: compose cannot read a Rust constant, and
//! the constant cannot read a file the binary is not deployed with. Divergence
//! is silent in the direction that costs the most. A venue marked parked here
//! but left in the default bring-up is *started* while every consumer is told
//! it is off by decision — so a genuinely broken collector reads as parked and
//! nobody looks. That is the same hide-a-fault error the parked marker exists to
//! remove, reintroduced one level up.
//!
//! **What this checks, stated narrowly, because the obvious summary overstates
//! it.** For each entry: the compose service exists, it declares *some*
//! `profiles:` block, none of those profiles is the one `collectors-up` enables,
//! and the venue is named in none of the Makefile's start lists. That is not the
//! same claim as "compose never starts it" — `make pyth-up` passes
//! `--profile pyth` explicitly and starts pyth while this file stays green. A
//! deliberate, named, opt-in start is exactly what a parked source is *for*;
//! what the test forbids is a source that starts as a side effect of the
//! ordinary bring-up.
//!
//! **Why the Makefile is read as well as compose.** Reading compose alone would
//! leave the check resting on `STARTED_PROFILE` being the complete set of
//! profiles the bring-up enables — a hand-copied fact about a different file
//! that nothing pinned, so it could silently narrow the moment `collectors-up`
//! grew a second `--profile`. Worse, compose's profile resolution is
//! command-dependent, so "behind a profile" does not by itself establish "not
//! started": the bring-up also names services *explicitly*, and reasoning about
//! whether that auto-enables their profiles is exactly the kind of argument a
//! test should make unnecessary. Checking the start lists directly settles it
//! without needing to know.
//!
//! **What this does NOT check.** It does not check the reverse direction — that
//! every profiled service is parked — because "behind a profile" and "parked"
//! are genuinely different properties. The three keyed FX venues sit behind the
//! `fx` profile and are fully expected to run. Profiles also carry things that
//! are not sources at all (the bots, the taker). Closing that direction needs a
//! canonical list of every source, which no single artifact holds today; until
//! one exists, a new opt-in *source* can be added without declaring a park and
//! this file will not notice.
//!
//! **Why it compares text.** Compose is YAML and the Makefile is make; this
//! crate has a parser for neither, so both are read as source — with the
//! incidental benefit of failing if either file's shape drifts far enough that
//! the scan can no longer find what it needs, rather than passing vacuously.
//! [`the_extractors_actually_found_their_targets`] is what makes that explicit.

use dropset_feeds::PARKED_SOURCES;
use std::collections::{BTreeMap, BTreeSet};

const COMPOSE: &str = include_str!("../../infra/localnet/docker-compose.yml");
const MAKEFILE: &str = include_str!("../../Makefile");

/// The profile `make collectors-up` enables, so a service behind it IS started
/// by the default bring-up and cannot be described as parked.
const STARTED_PROFILE: &str = "fx";

/// The Makefile variables naming services the ordinary bring-up starts.
///
/// `PYTH_SERVICES` is deliberately absent: it is removal-only, named by
/// `collectors-down` so a teardown cannot lose the container, and by no target
/// that starts anything. That asymmetry is the whole shape of a park, so
/// treating it as a start list would make this test contradict the thing it
/// exists to verify.
const START_LISTS: &[&str] = &["KEYLESS_SERVICES", "KEYED_SERVICES"];

/// Every compose service that declares a `profiles:` block, mapped to the
/// profiles it lists.
///
/// Scoped to the top-level `services:` block: service keys sit at two-space
/// indentation and their fields at four, which is what the repo's `yamllint`
/// configuration enforces — so the indentation is a pinned property of the file
/// rather than a guess about its formatting. Without the `services:` scope, a
/// two-space key under `volumes:` or `networks:` would read as a service and
/// weaken [`the_parked_set_is_populated`] to "names a two-space key somewhere in
/// the file".
fn service_profiles() -> BTreeMap<String, BTreeSet<String>> {
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut service: Option<String> = None;
    let mut in_profiles = false;

    for line in service_lines() {
        let trimmed = line.trim_end();
        let indent = trimmed.len() - trimmed.trim_start().len();

        if indent == 2 {
            if let Some(name) = trimmed.trim().strip_suffix(':') {
                service = Some(name.to_string());
                in_profiles = false;
                continue;
            }
        }

        // A sequence entry under `profiles:`. This is tested BEFORE the field
        // branch below and the ordering is load-bearing: the repo's yamllint
        // style keeps a sequence FLAT with its parent key, so `- 'pyth'` sits at
        // the same four-space indent as `profiles:` itself. Read as a field
        // first, it would clear the flag before ever being recognized as an
        // entry — which is what it did, and what the vacuity guard below caught.
        if in_profiles {
            if let Some(entry) = trimmed.trim().strip_prefix("- ") {
                if let Some(name) = service.as_ref() {
                    found
                        .entry(name.clone())
                        .or_default()
                        .insert(unquote(entry).to_string());
                }
                continue;
            }
        }

        if indent == 4 {
            in_profiles = trimmed.trim() == "profiles:";
        }
    }
    found
}

/// Every service key in the compose file's top-level `services:` block.
fn service_keys() -> BTreeSet<String> {
    service_lines()
        .filter(|l| l.len() - l.trim_start().len() == 2)
        .filter_map(|l| l.trim().strip_suffix(':'))
        .map(str::to_string)
        .collect()
}

/// The lines of the top-level `services:` block, comments and blanks dropped.
///
/// One owner for the scoping, so the two extractors above cannot drift the way
/// an inline copy in each of them did.
fn service_lines() -> impl Iterator<Item = &'static str> {
    let mut in_services = false;
    COMPOSE.lines().filter(move |line| {
        let trimmed = line.trim_end();
        if trimmed.is_empty() || trimmed.trim_start().starts_with('#') {
            return false;
        }
        // A top-level key closes the previous block and may open ours.
        if trimmed.len() == trimmed.trim_start().len() {
            in_services = trimmed.trim() == "services:";
            return false;
        }
        in_services
    })
}

/// Strip the quotes the repo's YAML style puts around a string. Single quotes
/// are what the style uses; double quotes are tolerated so a hand-edited entry
/// does not read as a distinct profile name.
fn unquote(value: &str) -> &str {
    value.trim().trim_matches('\'').trim_matches('"')
}

/// The services named by a `NAME = a b c` Makefile assignment, or `None` if the
/// variable is absent.
fn make_list(name: &str) -> Option<BTreeSet<String>> {
    MAKEFILE
        .lines()
        .find_map(|line| line.strip_prefix(name)?.trim_start().strip_prefix('='))
        .map(|value| {
            value
                .split_whitespace()
                .filter(|t| !t.starts_with('$'))
                .map(str::to_string)
                .collect()
        })
}

/// A parked venue's compose service must be behind at least one profile, none of
/// those may be the profile the default bring-up enables, and the venue must not
/// be named in any of the Makefile's start lists.
///
/// The three halves fail for different reasons. No profile at all means the
/// service is in a plain `docker compose up`; being behind [`STARTED_PROFILE`]
/// means `collectors-up` enables it explicitly; being in a start list means the
/// bring-up names it directly, whatever its profiles say. Any of the three
/// leaves the source running while every reader is told it is off by decision.
#[test]
fn every_parked_source_is_held_out_of_the_default_bring_up() {
    let profiles = service_profiles();
    for park in PARKED_SOURCES {
        let listed = profiles.get(park.venue).unwrap_or_else(|| {
            panic!(
                "`{}` is parked but its compose service declares no `profiles:` \
                 block, so the default bring-up starts it — either put it behind \
                 a profile or drop its PARKED_SOURCES entry",
                park.venue
            )
        });
        assert!(
            !listed.contains(STARTED_PROFILE),
            "`{}` is parked but sits behind the `{}` profile, which \
             `make collectors-up` enables — so it is started, not parked: {:?}",
            park.venue,
            STARTED_PROFILE,
            listed,
        );
        for list in START_LISTS {
            let services = make_list(list)
                .unwrap_or_else(|| panic!("no `{list} =` assignment in the Makefile"));
            assert!(
                !services.contains(park.venue),
                "`{}` is parked but the Makefile's {} names it, so the ordinary \
                 bring-up starts it regardless of its compose profiles: {:?}",
                park.venue,
                list,
                services,
            );
        }
    }
}

/// Guard against the checks above passing vacuously — if either scan stops
/// matching its file's shape, this is what fails instead of a silent green.
#[test]
fn the_extractors_actually_found_their_targets() {
    let profiles = service_profiles();
    // Both profiles this file reasons about must be present. This is stricter
    // than a bare count and it is the discriminating form: a count tolerates the
    // scan losing whichever profile the assertions actually turn on.
    let all: BTreeSet<&String> = profiles.values().flatten().collect();
    for required in [STARTED_PROFILE, "pyth"] {
        assert!(
            all.contains(&required.to_string()),
            "no service found behind the `{required}` profile, so the \
             not-started assertions cannot be discriminating: {profiles:?}",
        );
    }
    for list in START_LISTS {
        let services =
            make_list(list).unwrap_or_else(|| panic!("no `{list} =` assignment in the Makefile"));
        assert!(
            !services.is_empty(),
            "the Makefile's {list} parsed as empty; the scan has probably \
             stopped matching the file's shape",
        );
    }
}

/// The parked set is not empty, and every venue in it names a real compose
/// service.
///
/// Without this, emptying `PARKED_SOURCES` would make the agreement test above
/// pass by having nothing to check — and a typo'd venue token would too, since
/// the panic it produces reads like a compose problem.
///
/// Note this asserts a contract the `venue` field's doc also records: the token
/// must be both a bare venue token and a compose service key. Those two
/// vocabularies overlap by convention, not by rule — `coinbase-ticker` is a
/// compose service that is no venue token — so a park on a venue whose service
/// is spelled differently fails here and wants an explicit mapping.
#[test]
fn the_parked_set_is_populated() {
    assert!(
        !PARKED_SOURCES.is_empty(),
        "PARKED_SOURCES is empty; if the last park was lifted, this test and \
         the agreement above should be reconsidered rather than left vacuous",
    );
    let services = service_keys();
    for park in PARKED_SOURCES {
        assert!(
            services.contains(park.venue),
            "`{}` is parked but names no compose service — check the venue \
             token against the service key",
            park.venue,
        );
    }
}
