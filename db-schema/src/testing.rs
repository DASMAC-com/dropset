//! Support for the workspace's container-backed Postgres tests.
//!
//! Every crate that tests against a throwaway Postgres starts it and resolves
//! its mapped port the same way, and that lookup fails intermittently; if only
//! one copy handled it, the others would flake on a different test each run. It
//! lives here, beside [`POSTGRES_IMAGE_TAG`],
//! because this crate already sits below every one of those consumers.
//!
//! Behind the `test-support` feature, which only dev-dependencies enable, so no
//! deployed binary links testcontainers.

use crate::POSTGRES_IMAGE_TAG;
use std::time::Duration;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::{
    runners::AsyncRunner, ContainerAsync, ImageExt, TestcontainersError,
};

/// Start a throwaway Postgres on [`POSTGRES_IMAGE_TAG`] and return it with the
/// host port Docker mapped to its 5432. Nothing is migrated.
///
/// Keep the returned container bound for the life of the test — dropping it
/// stops the container.
///
/// The port lookup intermittently fails with `PortNotExposed`, on a different
/// test each run and more often under parallel load, and retrying it on the
/// same container does not help. On Docker Desktop the Docker daemon runs in a
/// VM and picks an ephemeral host port it cannot see is free on the host;
/// when the host-side forwarder then fails to bind it (`address already in
/// use` in the backend log), it gives up after a few milliseconds and the
/// container's port binding stays empty for its whole life. A fresh container
/// gets a fresh port, so after a short lookup retry — which still covers a
/// mapping published merely late — the container is discarded and another
/// started. Any other error is a real failure and panics on the spot.
///
/// Each discard is noted on stderr, which a test harness captures: nextest and
/// `cargo test` show it only for a failing test or under `--no-capture`, so a
/// restart that recovered leaves no trace in a passing run's log.
pub async fn start_postgres() -> (ContainerAsync<Postgres>, u16) {
    const CONTAINERS: u32 = 3;
    const LOOKUPS: u32 = 5;
    const BACKOFF: Duration = Duration::from_millis(200);
    for container_attempt in 1..=CONTAINERS {
        let container = Postgres::default()
            .with_tag(POSTGRES_IMAGE_TAG)
            .start()
            .await
            .expect("start postgres container");
        for lookup in 1..=LOOKUPS {
            match container.get_host_port_ipv4(5432).await {
                Ok(port) => return (container, port),
                Err(TestcontainersError::PortNotExposed { .. }) if lookup < LOOKUPS => {
                    tokio::time::sleep(BACKOFF).await;
                }
                Err(TestcontainersError::PortNotExposed { .. })
                    if container_attempt < CONTAINERS =>
                {
                    eprintln!(
                        "postgres container {} never published 5432 \
                         (container {container_attempt}/{CONTAINERS}); starting another",
                        container.id()
                    );
                }
                Err(e) => panic!(
                    "resolve mapped port (container {container_attempt}/{CONTAINERS}, \
                     lookup {lookup}/{LOOKUPS}): {e:?}"
                ),
            }
        }
    }
    unreachable!("the final container's final lookup either returns or panics")
}
