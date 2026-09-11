//! Every parked source exists twice — once as a `dropset_feeds::PARKED_SOURCES`
//! entry, once as a `docker-compose.yml` service held behind a profile the
//! default bring-up does not enable — and this pins the two together.
//!
//! **Why that needs a test.** The two say the same thing to different readers
//! and neither can derive the other: compose cannot read a Rust constant, and
//! the constant cannot read a file the binary is not deployed with. Divergence
//! is silent in the direction that costs the most. A venue marked parked here
//! but left in the default compose set is *started* while every consumer is told
//! it is off by decision — so a genuinely broken collector reads as parked and
//! nobody looks. That is the same hide-a-fault error the parked marker exists to
//! remove, reintroduced one level up.
//!
//! **What this does NOT check, stated so nobody credits it with more.** It does
//! not check the reverse direction — that every profiled service is parked —
//! because "behind a profile" and "parked" are genuinely different properties
//! here. The three keyed FX venues sit behind the `fx` profile and are fully
//! expected to run: `make collectors-up` enables that profile and starts them
//! from the secrets enclave. Profiles also carry things that are not sources at
//! all (the bots, the taker). Closing that direction needs a canonical list of
//! every source, which no single artifact holds today; until one exists, a new
//! opt-in *source* can be added without declaring a park and this file will not
//! notice.
//!
//! **Why it compares text.** Compose is YAML and this crate has no YAML parser
//! in its dependency tree, so the file is read as source — with the incidental
//! benefit of failing if its shape drifts far enough that the scan can no longer
//! find a service, rather than passing vacuously.
//! [`the_extractor_actually_found_the_profiles`] is what makes that explicit.

use dropset_feeds::PARKED_SOURCES;
use std::collections::{BTreeMap, BTreeSet};

const COMPOSE: &str = include_str!("../../infra/localnet/docker-compose.yml");

/// The profile `make collectors-up` enables, so a service behind it IS started
/// by the default bring-up and cannot be described as parked.
const STARTED_PROFILE: &str = "fx";

/// Every compose service that declares a `profiles:` block, mapped to the
/// profiles it lists.
///
/// Services are keyed at two-space indentation under `services:` and their
/// fields at four, which is what the repo's `yamllint` configuration enforces —
/// so the indentation is a pinned property of the file rather than a guess about
/// its formatting.
fn service_profiles() -> BTreeMap<String, BTreeSet<String>> {
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut service: Option<String> = None;
    let mut in_profiles = false;

    for line in COMPOSE.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() || trimmed.trim_start().starts_with('#') {
            continue;
        }
        let indent = trimmed.len() - trimmed.trim_start().len();

        // A service key: exactly two spaces in, ending in a colon.
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
        // entry — which is exactly what it did, and what the vacuity guard
        // below caught.
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

        // A field of the current service, at four spaces.
        if indent == 4 {
            in_profiles = trimmed.trim() == "profiles:";
            continue;
        }
    }
    found
}

/// Strip the single quotes the repo's YAML style puts around every string.
fn unquote(value: &str) -> &str {
    value.trim().trim_matches('\'').trim_matches('"')
}

/// A parked venue's compose service must be behind at least one profile, and
/// none of those may be the profile the default bring-up enables.
///
/// Both halves matter and they fail for different reasons. No profile at all
/// means the service is in a plain `docker compose up` and in `collectors-up`,
/// so it runs; being behind [`STARTED_PROFILE`] means `collectors-up` enables it
/// explicitly. Either way the source is running while every reader is told it is
/// off by decision.
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
    }
}

/// Guard against the check above passing vacuously — if the scan stops matching
/// the compose file's shape, this is what fails instead of a silent green.
#[test]
fn the_extractor_actually_found_the_profiles() {
    let profiles = service_profiles();
    // A floor rather than an equality, so adding a profiled service does not
    // fail this. Today: the three keyed FX venues, pyth, and the bot/taker
    // services.
    assert!(
        profiles.len() >= 4,
        "found `profiles:` for only {} compose services; the scan has probably \
         stopped matching the file's shape: {:?}",
        profiles.len(),
        profiles,
    );
    assert!(
        profiles.values().any(|p| p.contains(STARTED_PROFILE)),
        "no service found behind the `{STARTED_PROFILE}` profile, so the \
         not-started assertion above cannot be discriminating: {profiles:?}",
    );
}

/// The parked set is not empty, and every venue in it names a real compose
/// service.
///
/// Without this, emptying `PARKED_SOURCES` would make the agreement test above
/// pass by having nothing to check — and a typo'd venue token would too, since
/// the panic message it produces reads like a compose problem.
#[test]
fn the_parked_set_is_populated() {
    assert!(
        !PARKED_SOURCES.is_empty(),
        "PARKED_SOURCES is empty; if the last park was lifted, this test and \
         the agreement above should be reconsidered rather than left vacuous",
    );
    let services: BTreeSet<&str> = COMPOSE
        .lines()
        .filter(|l| l.len() - l.trim_start().len() == 2)
        .filter_map(|l| l.trim().strip_suffix(':'))
        .collect();
    for park in PARKED_SOURCES {
        assert!(
            services.contains(park.venue),
            "`{}` is parked but names no compose service — check the venue \
             token against the service key",
            park.venue,
        );
    }
}
