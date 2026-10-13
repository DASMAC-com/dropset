//! Support for the workspace's container-backed Postgres tests.
//!
//! Every crate that tests against a throwaway Postgres resolves the container's
//! mapped port the same way, and the lookup has a startup race that one copy
//! absorbing it and the others not would leave flaking on a different test each
//! run. It lives here, beside [`POSTGRES_IMAGE_TAG`](crate::POSTGRES_IMAGE_TAG),
//! because this crate already sits below every one of those consumers.
//!
//! Behind the `test-support` feature, which only dev-dependencies enable, so no
//! deployed binary links testcontainers.

use std::time::Duration;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::{ContainerAsync, TestcontainersError};

/// The host port Docker mapped to the container's 5432.
///
/// This lookup intermittently fails with `PortNotExposed`, on a different test
/// each run. `start` has already waited for Postgres to report ready, so the
/// likely cause is Docker publishing the port mapping late rather than a broken
/// container — the flake has not been reproduced on demand. It is absorbed
/// here: each attempt re-inspects the container, ten attempts 200ms apart
/// (about 1.8s in all), and only that one error is retried — anything else is a
/// real failure and panics on the spot. A `PortNotExposed` that outlasts every
/// attempt is no longer a short race; read it as a container that died or
/// never bound IPv4.
pub async fn mapped_port(container: &ContainerAsync<Postgres>) -> u16 {
    const ATTEMPTS: u32 = 10;
    const BACKOFF: Duration = Duration::from_millis(200);
    for attempt in 1..=ATTEMPTS {
        match container.get_host_port_ipv4(5432).await {
            Ok(port) => return port,
            Err(TestcontainersError::PortNotExposed { .. }) if attempt < ATTEMPTS => {
                tokio::time::sleep(BACKOFF).await;
            }
            Err(e) => panic!("resolve mapped port (attempt {attempt}/{ATTEMPTS}): {e:?}"),
        }
    }
    unreachable!("the final attempt either returns or panics")
}
