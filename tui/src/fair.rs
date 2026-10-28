// cspell:word hostaddr

//! The fair-price estimator's current value — one of the passive center's
//! sources.
//!
//! The TUI otherwise has no market-data dependency, so this read is optional:
//! with no database configured the passive form centers on a typed price, the
//! vault's live reference or the book mid, as it always has. `make tui`,
//! `make tui-mainnet` and `make mainnet-demo` configure the local reader by
//! default. Once one is
//! configured the form's chooser offers the estimator's fair beside the
//! [`SPOT_SOURCE`] spot — a paused, stale, static-pegged or unreadable fair is
//! never offered, and a blank typed center refuses rather than quietly falling
//! back to an older one.
//!
//! The read goes through the dashboards' read-only role, refused by name
//! otherwise, on a session pinned read-only besides. It reuses market-data's
//! [`FairPriceSource`] and [`FairPriceRow::fair_value`], so what reads as
//! paused or malformed here is exactly what the maker bot refuses.
//!
//! The same database also serves the newest on-chain spot print
//! ([`FairDb::spot`]), which the passive form offers beside the fair and
//! centers on only while it is inside [`MAX_SPOT_AGE_SECS`].

use anyhow::{bail, Context, Result};
use dropset_market_data::estimator::published_product;
use dropset_market_data::fair_price::{
    FairPriceRow, FairPriceSource, MAX_PUBLISHED_FAIR_AGE, MAX_PUBLISHED_FAIR_SKEW,
};
use dropset_market_data::tick_store::TickStoreReader;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};
use sqlx::PgPool;
use std::future::Future;
use std::str::FromStr;
use std::time::Duration;

/// The environment variable naming the fair-price database. Optional.
pub const DATABASE_URL_ENV: &str = "DROPSET_TUI_DATABASE_URL";

/// The read-only role 0002 creates — the one the dashboards log in as.
pub const READER_ROLE: &str = "dropset_ro";

/// How long one read may take, connect included, before it reads as
/// unavailable — the prompt waits on it.
const READ_TIMEOUT: Duration = Duration::from_secs(3);

/// A configured fair-price database.
///
/// No derived `Debug`: the connect options carry the password, and a
/// `{:?}` would put it in the log pane and its on-disk mirror.
#[derive(Clone)]
pub struct FairDb {
    options: PgConnectOptions,
}

impl std::fmt::Debug for FairDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FairDb")
            .field("host", &self.options.get_host())
            .field("user", &self.options.get_username())
            .finish_non_exhaustive()
    }
}

/// Whether `host` — the one sqlx will dial, after any `host=` / `hostaddr=`
/// override — is a loopback address, where an unverified connection has no
/// path to intercept. A Unix-socket URL keeps sqlx's default `localhost`.
fn is_local(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

impl FairDb {
    /// The database [`DATABASE_URL_ENV`] names, `None` when it is unset.
    ///
    /// # Errors
    ///
    /// When it is set but unparseable, logs in as any role but the reader,
    /// or names a remote host without a TLS connection verified to that host
    /// (`sslmode=verify-full`; `verify-ca` checks no hostname, so any
    /// certificate from a trusted CA would pass). The fair it reads becomes the
    /// center a quote key signs, so a connection anyone on the path could
    /// answer is refused rather than trusted.
    pub fn from_env() -> Result<Option<Self>> {
        match std::env::var(DATABASE_URL_ENV) {
            Ok(url) if !url.is_empty() => Self::from_url(&url).map(Some),
            _ => Ok(None),
        }
    }

    fn from_url(url: &str) -> Result<Self> {
        let options = PgConnectOptions::from_str(url)
            .with_context(|| format!("{DATABASE_URL_ENV} is not a Postgres URL"))?;
        if options.get_username() != READER_ROLE {
            bail!(
                "{DATABASE_URL_ENV} must log in as {READER_ROLE}, the read-only role — not {}",
                options.get_username()
            );
        }
        let verified = matches!(options.get_ssl_mode(), PgSslMode::VerifyFull);
        if !is_local(options.get_host()) && !verified {
            bail!(
                "{DATABASE_URL_ENV} names the remote host {} — add sslmode=verify-full \
                 (and sslrootcert=<CA file> for a private CA)",
                options.get_host()
            );
        }
        Ok(Self {
            options: options.options([("default_transaction_read_only", "on")]),
        })
    }

    /// The newest fair for the market whose base is `base_symbol`.
    pub fn read(&self, base_symbol: &str) -> Fair {
        let Some(product) = published_product(base_symbol) else {
            return Fair::Unpublished;
        };
        match self.latest(product) {
            Ok(rows) => classify(
                product,
                rows.iter().find(|r| r.product_id == product),
                dropset_sdk::time::now_unix(),
            ),
            Err(e) => Fair::Unread(format!("{product}: the read failed — {e:#}")),
        }
    }

    /// The newest [`SPOT_SOURCE`] print for the market whose base is
    /// `base_symbol`, under the estimator's product id — `None` when there is
    /// none, it does not decode as a price, it is stamped further ahead of
    /// this clock than `MAX_PUBLISHED_FAIR_SKEW` (a skewed row would read as
    /// fresh until the clock caught up), or the read fails. Shown beside the
    /// fair and offered as a center, so a miss is one source fewer rather
    /// than an error.
    pub fn spot(&self, base_symbol: &str) -> Option<Spot> {
        let product = published_product(base_symbol)?;
        let rows = self
            .query(|pool| async move {
                TickStoreReader::new(
                    pool,
                    vec![SPOT_SOURCE.to_string()],
                    vec![product.to_string()],
                )
                .latest()
                .await
            })
            .ok()?;
        let row = rows.into_iter().find(|r| r.product_id == product)?;
        spot_from(row.price, row.observed_at, dropset_sdk::time::now_unix())
    }

    fn latest(&self, product: &str) -> Result<Vec<FairPriceRow>> {
        self.query(|pool| async move {
            FairPriceSource::new("tui", pool, vec![product.to_string()])
                .latest()
                .await
        })
    }

    /// Run `read` on a one-connection pool, the whole of it — connect
    /// included — inside [`READ_TIMEOUT`].
    fn query<T, F, Fut>(&self, read: F) -> Result<T>
    where
        F: FnOnce(PgPool) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let out = runtime.block_on(async {
            tokio::time::timeout(READ_TIMEOUT, async {
                let pool = PgPoolOptions::new()
                    .max_connections(1)
                    .connect_with(self.options.clone())
                    .await?;
                let out = read(pool.clone()).await;
                pool.close().await;
                out
            })
            .await
            .context("timed out")?
        });
        // A timed-out hostname lookup still holds a blocking-pool thread,
        // which a plain drop would wait on, past the timeout, on the UI
        // thread. Abandon it instead.
        runtime.shutdown_background();
        out
    }
}

/// The `spot_ticks` source the passive form offers beside the fair: the
/// on-chain venue the market's flow is likeliest to arb against.
pub const SPOT_SOURCE: &str = "orca";

/// The oldest [`Spot`] a center may take: the estimator's default bound for a
/// tape source (`FairValueConfig`'s `leg_stale.tape`), so a pool print the
/// estimator would drop as a leg is not centered on either.
pub const MAX_SPOT_AGE_SECS: i64 = 5 * 60;

/// [`FairDb::spot`]'s verdict on one print observed at `observed_at`, at
/// `now_unix` — split out so it is testable without a database.
fn spot_from(price: f64, observed_at: i64, now_unix: i64) -> Option<Spot> {
    let ahead = observed_at.saturating_sub(now_unix);
    if !(price.is_finite() && price > 0.0) || ahead > MAX_PUBLISHED_FAIR_SKEW.as_secs() as i64 {
        return None;
    }
    Some(Spot {
        price,
        // A writer clock a little ahead reads as fresh, not negative.
        age_secs: now_unix.saturating_sub(observed_at).max(0),
    })
}

/// The `anchor` a `fair_price` row names when its fair is the configured
/// constant peg rather than a market read — never offered as a center.
const STATIC_ANCHOR: &str = "static";

/// A [`SPOT_SOURCE`] print, human quote-per-base.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub price: f64,
    pub age_secs: i64,
}

impl Spot {
    /// Whether a center may take this print — inside [`MAX_SPOT_AGE_SECS`].
    pub fn is_fresh(self) -> bool {
        self.age_secs <= MAX_SPOT_AGE_SECS
    }
}

/// The estimator's verdict on one market, as of one read.
#[derive(Clone, Debug, PartialEq)]
pub enum Fair {
    /// No database configured — the blank center is the live reference or
    /// the book mid.
    Off,
    /// The estimator publishes nothing for this market.
    Unpublished,
    /// A fresh fair, human quote-per-base.
    Live {
        product: &'static str,
        price: f64,
        age_secs: u64,
        /// The composition regime, named when it is not `normal`.
        regime: String,
        /// What the fair is anchored on — `static` for the peg.
        anchor: String,
    },
    /// The estimator is up and says not to quote: shown, never centered on.
    Paused {
        product: &'static str,
        regime: String,
    },
    /// The newest row is older than `MAX_PUBLISHED_FAIR_AGE`.
    Stale {
        product: &'static str,
        age_secs: u64,
    },
    /// The database answered with no usable row — absent, future-stamped or
    /// malformed.
    Unusable(String),
    /// The database did not answer — so the spot is not asked either.
    Unread(String),
}

impl Fair {
    /// The fresh fair's price, when there is one — the peg's included.
    pub fn price(&self) -> Option<f64> {
        match self {
            Fair::Live { price, .. } => Some(*price),
            _ => None,
        }
    }

    /// Whether the fair is the constant peg rather than a market read.
    pub fn is_static(&self) -> bool {
        matches!(self, Fair::Live { anchor, .. } if anchor == STATIC_ANCHOR)
    }

    /// Whether a blank center must come from a chosen source rather than the
    /// vault — an estimator is configured for this market.
    pub fn governs(&self) -> bool {
        !matches!(self, Fair::Off | Fair::Unpublished)
    }

    /// One line for the form and the refusal.
    pub fn describe(&self) -> String {
        match self {
            Fair::Off => "no fair-price database".to_string(),
            Fair::Unpublished => "the estimator publishes no fair for this market".to_string(),
            Fair::Live {
                product,
                price,
                age_secs,
                regime,
                ..
            } => {
                let regime = if regime == "normal" {
                    String::new()
                } else {
                    format!(", {regime}")
                };
                format!(
                    "fair {} ({product}, {age_secs} s old{regime})",
                    crate::book::fmt_price(*price)
                )
            }
            Fair::Paused { product, regime } => {
                format!("the estimator is paused on {product} ({regime})")
            }
            Fair::Stale { product, age_secs } => {
                format!("{product}'s row is {age_secs} s old — is the estimator running?")
            }
            Fair::Unusable(why) | Fair::Unread(why) => format!("no usable fair: {why}"),
        }
    }
}

/// [`FairDb::read`]'s verdict over the newest `row` for `product` at
/// `now_unix` — split out so it is testable without a database.
fn classify(product: &'static str, row: Option<&FairPriceRow>, now_unix: i64) -> Fair {
    let Some(row) = row else {
        return Fair::Unusable(format!("{product} has no published row"));
    };
    if row.future_stamped(now_unix) {
        return Fair::Unusable(format!("{product}'s row is stamped ahead of this clock"));
    }
    let age = row.age(now_unix);
    if age > MAX_PUBLISHED_FAIR_AGE {
        return Fair::Stale {
            product,
            age_secs: age.as_secs(),
        };
    }
    match row.fair_value() {
        Err(e) => Fair::Unusable(format!("{product}: {e}")),
        Ok(fv) => match fv.fair {
            Some(price) => Fair::Live {
                product,
                price,
                age_secs: age.as_secs(),
                regime: row.regime.clone(),
                anchor: row.anchor.clone(),
            },
            None => Fair::Paused {
                product,
                regime: row.regime.clone(),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn row(fair: Option<f64>, regime: &str, health: &str, ts: i64) -> FairPriceRow {
        FairPriceRow {
            ts,
            product_id: "TGBP-USDC".into(),
            fair,
            anchor: "fx".into(),
            regime: regime.into(),
            degrade: None,
            health: health.into(),
            basis: Some(1.0),
            basis_age_secs: Some(1),
            basis_outlier: false,
            uncertain: false,
            basis_breach: false,
            usdc_breach: false,
            fx_tape_live: Some(true),
        }
    }

    #[test]
    fn a_fresh_row_seeds_and_a_paused_one_never_does() {
        let live = classify(
            "TGBP-USDC",
            Some(&row(Some(1.324), "normal", "ok", NOW - 2)),
            NOW,
        );
        assert_eq!(live.price(), Some(1.324), "{live:?}");
        assert!(live.describe().contains("2 s old"));
        let paused = classify("TGBP-USDC", Some(&row(None, "paused", "pause", NOW)), NOW);
        assert!(matches!(paused, Fair::Paused { .. }), "{paused:?}");
        assert_eq!(paused.price(), None);
        assert!(paused.governs());
        // The peg is a fresh fair, flagged so the chooser never offers it.
        let mut pegged = row(Some(1.0), "normal", "ok", NOW);
        pegged.anchor = "static".into();
        let pegged = classify("TGBP-USDC", Some(&pegged), NOW);
        assert!(pegged.is_static() && !live.is_static(), "{pegged:?}");
    }

    #[test]
    fn a_missing_stale_or_future_row_is_unusable() {
        let unusable = |f: Fair| matches!(f, Fair::Unusable(_));
        assert!(unusable(classify("TGBP-USDC", None, NOW)));
        let stale = NOW - MAX_PUBLISHED_FAIR_AGE.as_secs() as i64 - 1;
        let stale = classify(
            "TGBP-USDC",
            Some(&row(Some(1.3), "normal", "ok", stale)),
            NOW,
        );
        assert!(matches!(stale, Fair::Stale { .. }), "{stale:?}");
        assert_eq!(stale.price(), None);
        assert!(unusable(classify(
            "TGBP-USDC",
            Some(&row(Some(1.3), "normal", "ok", NOW + 600)),
            NOW
        )));
        // A fair outside a paused regime with no price is malformed, not paused.
        assert!(unusable(classify(
            "TGBP-USDC",
            Some(&row(None, "normal", "ok", NOW)),
            NOW
        )));
    }

    #[test]
    fn only_the_reader_role_is_accepted() {
        assert!(FairDb::from_url("postgres://dropset_ro:pw@127.0.0.1:5432/dropset").is_ok());
        let err = FairDb::from_url("postgres://dropset:pw@127.0.0.1:5432/dropset")
            .unwrap_err()
            .to_string();
        assert!(err.contains(READER_ROLE), "{err}");
        assert!(FairDb::from_url("not a url").is_err());
        assert!(!Fair::Off.governs() && !Fair::Unpublished.governs());
    }

    #[test]
    fn a_remote_host_needs_a_verified_connection() {
        let remote = "postgres://dropset_ro:pw@db.example.com:5432/dropset";
        let err = FairDb::from_url(remote).unwrap_err().to_string();
        assert!(err.contains("sslmode=verify-full"), "{err}");
        // Encryption without verification answers no one on the path.
        assert!(FairDb::from_url(&format!("{remote}?sslmode=require")).is_err());
        assert!(FairDb::from_url(&format!("{remote}?sslmode=verify-ca")).is_err());
        // The gate judges the host sqlx dials, after a query override.
        let detour = "postgres://dropset_ro:pw@localhost/dropset?host=db.example.com";
        assert!(FairDb::from_url(detour).is_err());
        assert!(FairDb::from_url(&format!("{remote}?sslmode=verify-full")).is_ok());
        assert!(FairDb::from_url("postgres://dropset_ro:pw@localhost/dropset").is_ok());
    }

    #[test]
    fn a_spot_stamped_past_the_skew_or_unpriced_is_no_spot() {
        let skew = MAX_PUBLISHED_FAIR_SKEW.as_secs() as i64;
        assert_eq!(
            spot_from(1.32, NOW - 40, NOW),
            Some(Spot {
                price: 1.32,
                age_secs: 40
            })
        );
        // Inside the skew it reads as fresh; one second past it, as nothing.
        assert_eq!(spot_from(1.32, NOW + skew, NOW).unwrap().age_secs, 0);
        assert_eq!(spot_from(1.32, NOW + skew + 1, NOW), None);
        assert_eq!(spot_from(0.0, NOW, NOW), None);
        assert_eq!(spot_from(f64::NAN, NOW, NOW), None);
        // The freshness bound is inclusive.
        assert!(spot_from(1.0, NOW - MAX_SPOT_AGE_SECS, NOW)
            .unwrap()
            .is_fresh());
        assert!(!spot_from(1.0, NOW - MAX_SPOT_AGE_SECS - 1, NOW)
            .unwrap()
            .is_fresh());
    }

    #[test]
    fn the_password_never_reaches_a_debug_and_an_unpublished_market_never_connects() {
        let db = FairDb::from_url("postgres://dropset_ro:SECRETpw@127.0.0.1:5432/dropset").unwrap();
        assert!(!format!("{db:?}").contains("SECRETpw"));
        // A market the estimator does not roster returns before connecting.
        assert_eq!(db.read("NOPE"), Fair::Unpublished);
    }
}
