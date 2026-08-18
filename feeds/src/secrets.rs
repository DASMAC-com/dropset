//! Secret resolution — one interface, one backend per store.
//!
//! Every credential a feed needs is named **canonically**, as
//! `<provider>/<secret>`: the party that *issued* the credential, then which of
//! its credentials this is. `oanda/api-key` is the OANDA API key no matter who
//! reads it, so a key shared by the collectors and a bot has exactly one name,
//! one entry per store, and one place to rotate. Naming a secret after its
//! consumer (`market-data/oanda-key`) would have to be renamed the moment a
//! second consumer appeared, and a rename that lands in one store but not the
//! other is a silent outage.
//!
//! The canonical name is not translated — each backend only **prefixes** it:
//!
//! | store              | key for `oanda/api-key`      |
//! |--------------------|------------------------------|
//! | process env        | `OANDA_API_KEY`              |
//! | 1Password          | `op://<vault>/oanda/api-key` |
//! | AWS Secrets Manager| `dropset/oanda/api-key`      |
//!
//! 1Password is the **local mock of AWS Secrets Manager**: same names, same
//! layout, different store, so a collector that resolves locally resolves the
//! same way once deployed. The 1Password backend ships here; the Secrets
//! Manager one lands with the hosted deploy and is a third [`Backend`] pushed
//! onto the same chain — no call site changes.
//!
//! **Why the name maps onto 1Password with no escaping at all.** A secret
//! reference parses as `op://<vault>/<item>/[<section>/]<field>`, so a slash in
//! an *item title* is not quotable — it re-segments the reference and the item
//! stops resolving. That rules out storing a hierarchical name as a title. It
//! does not rule out hierarchy: an item's **fields** are addressable by label,
//! which gives exactly the two levels `<provider>/<secret>` needs. So the
//! provider is the item, the secret is the field, and the canonical name is
//! already a valid reference tail.
//!
//! Resolution is **fetch-once-at-startup** — a backend is consulted while a
//! binary is wiring itself up, never per request, so the 1Password backend's
//! subprocess and the eventual Secrets Manager round trip stay off every hot
//! path.

use anyhow::{anyhow, bail, Context, Result};
use std::process::Command;

/// Environment variable naming the 1Password vault to resolve against. Unset
/// means the 1Password backend is not wired up and only the environment is
/// consulted, which is the CI and container case.
pub const VAULT_ENV: &str = "DROPSET_OP_VAULT";

/// Environment variable naming the 1Password account to resolve against.
/// Optional: `op` only needs it on a machine signed in to more than one
/// account, where a bare read cannot disambiguate and fails.
pub const ACCOUNT_ENV: &str = "DROPSET_OP_ACCOUNT";

/// The prefix a canonical name carries in AWS Secrets Manager, where the
/// account holds more than this one application's secrets. 1Password needs no
/// equivalent — there the vault *is* the scope.
pub const AWS_PREFIX: &str = "dropset";

/// One store a secret can be resolved from.
///
/// Object-safe on purpose: [`SecretProvider`] holds a chain of these, so
/// adding the Secrets Manager backend is a push onto that chain rather than a
/// change at any call site.
pub trait Backend: Send + Sync {
    /// How this backend is named in an error, e.g. `the environment`.
    fn describe(&self) -> String;

    /// The store-specific key `name` maps onto, for error messages — the whole
    /// point of the canonical scheme is that an operator can read this and go
    /// look the value up by hand.
    fn key_for(&self, name: &str) -> String;

    /// Resolve `name`, or `Ok(None)` when this store simply doesn't carry it.
    ///
    /// A missing secret is `Ok(None)` so the chain can fall through to the next
    /// backend; `Err` is reserved for a store that was supposed to answer and
    /// couldn't (`op` absent, not signed in, a vault that doesn't exist). That
    /// split is what keeps a broken 1Password setup from silently looking like
    /// an unset key.
    fn resolve(&self, name: &str) -> Result<Option<String>>;
}

/// The process environment. Always first in the chain: an explicitly exported
/// value is the override path, and it is what CI, the containers, and a
/// one-off `KEY=… cargo run` all use.
pub struct EnvBackend;

impl Backend for EnvBackend {
    fn describe(&self) -> String {
        "the environment".to_string()
    }

    fn key_for(&self, name: &str) -> String {
        env_var(name)
    }

    fn resolve(&self, name: &str) -> Result<Option<String>> {
        match std::env::var(env_var(name)) {
            // An **empty** value is treated as absent rather than as a secret,
            // so the chain falls through to 1Password instead of handing a
            // venue an empty key. The compose services pass credentials as
            // `${VAR:-}`, so an unset key arrives as an empty string rather
            // than as a missing variable — without this the local enclave
            // would be unreachable from a shell that had ever sourced them.
            Ok(v) if v.trim().is_empty() => Ok(None),
            Ok(v) => Ok(Some(v)),
            Err(_) => Ok(None),
        }
    }
}

/// A 1Password vault, read through the `op` CLI — the local mock of AWS
/// Secrets Manager.
///
/// The CLI is shelled out to rather than linked: `op` already owns the session,
/// the biometric unlock, and the account disambiguation, none of which this
/// process should reimplement to read three keys at startup.
pub struct OnePasswordBackend {
    vault: String,
    account: Option<String>,
}

impl OnePasswordBackend {
    /// Build a backend over `vault`, optionally pinned to `account`.
    pub fn new(vault: impl Into<String>, account: Option<String>) -> Self {
        Self {
            vault: vault.into(),
            account,
        }
    }

    /// The full secret reference for `name`, e.g.
    /// `op://<vault>/oanda/api-key`.
    pub fn reference(&self, name: &str) -> String {
        format!("op://{}/{name}", self.vault)
    }
}

impl Backend for OnePasswordBackend {
    fn describe(&self) -> String {
        format!("1Password vault {:?}", self.vault)
    }

    fn key_for(&self, name: &str) -> String {
        self.reference(name)
    }

    fn resolve(&self, name: &str) -> Result<Option<String>> {
        let reference = self.reference(name);
        let mut command = Command::new("op");
        if let Some(account) = &self.account {
            command.args(["--account", account]);
        }
        command.args(["read", &reference]);

        let output = command.output().with_context(|| {
            format!(
                "could not run `op` to read {reference} — is the 1Password CLI \
                 installed? Unset {VAULT_ENV} to resolve from the environment \
                 instead"
            )
        })?;

        if !output.status.success() {
            // `op` distinguishes "no such item/field" from every other failure
            // only in its message, so match on it: a missing secret falls
            // through to the next backend, while a broken session or an absent
            // vault is a hard error an operator has to fix.
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("isn't an item") || stderr.contains("does not have a field") {
                return Ok(None);
            }
            bail!("`op read {reference}` failed: {}", stderr.trim());
        }

        // `op read` terminates the value with a newline that is not part of it.
        let value = String::from_utf8(output.stdout)
            .with_context(|| format!("{reference} is not valid UTF-8"))?
            .trim_end_matches(['\r', '\n'])
            .to_string();

        if value.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(value))
    }
}

/// The chain of stores a secret is resolved from, in order.
pub struct SecretProvider {
    backends: Vec<Box<dyn Backend>>,
}

impl SecretProvider {
    /// Build the chain from the environment: the process environment first,
    /// then a 1Password vault when [`VAULT_ENV`] names one.
    ///
    /// The environment deliberately wins. It is both the override path (pin one
    /// key without touching the vault) and the no-1Password path — CI and the
    /// collector containers have no `op` and must never need one.
    pub fn from_env() -> Self {
        let mut backends: Vec<Box<dyn Backend>> = vec![Box::new(EnvBackend)];
        if let Some(vault) = non_empty(VAULT_ENV) {
            backends.push(Box::new(OnePasswordBackend::new(
                vault,
                non_empty(ACCOUNT_ENV),
            )));
        }
        Self { backends }
    }

    /// Build a chain over an explicit backend list (tests, and the hosted
    /// wiring once the Secrets Manager backend exists).
    pub fn new(backends: Vec<Box<dyn Backend>>) -> Self {
        Self { backends }
    }

    /// Resolve a canonical `<provider>/<secret>` name, consulting each backend
    /// in turn.
    ///
    /// Call this **once, at startup**. The error names every key that was
    /// tried, in every store's own spelling, because the failure an operator
    /// actually hits is "I put it somewhere, but not where this looked".
    pub fn resolve(&self, name: &str) -> Result<String> {
        validate_name(name)?;
        for backend in &self.backends {
            if let Some(value) = backend.resolve(name)? {
                return Ok(value);
            }
        }
        let tried = self
            .backends
            .iter()
            .map(|b| format!("{} ({})", b.key_for(name), b.describe()))
            .collect::<Vec<_>>()
            .join(", ");
        Err(anyhow!(
            "the secret {name:?} is not set — looked for {tried}"
        ))
    }
}

/// The environment variable a canonical name maps onto: upper case, with both
/// separators folded to `_`. `oanda/api-key` → `OANDA_API_KEY`.
pub fn env_var(name: &str) -> String {
    name.replace(['/', '-'], "_").to_uppercase()
}

/// The AWS Secrets Manager id a canonical name maps onto — the name itself,
/// under the application prefix. `oanda/api-key` →
/// `dropset/oanda/api-key`.
pub fn aws_secret_id(name: &str) -> String {
    format!("{AWS_PREFIX}/{name}")
}

/// Reject anything that is not a canonical `<provider>/<secret>` name.
///
/// The charset is the intersection of what all three stores accept unescaped,
/// so a name that passes here needs no per-store quoting. **A dot is
/// deliberately excluded**: it is legal in an AWS secret id, but admitting one
/// would make a name that looks like the dot-delimited flattening an earlier
/// design used, and the two schemes are not interchangeable. Exactly two
/// segments, because 1Password's reference grammar offers exactly two
/// addressable levels below a vault (item, then field).
pub fn validate_name(name: &str) -> Result<(&str, &str)> {
    let (provider, secret) = name
        .split_once('/')
        .ok_or_else(|| anyhow!("{name:?} is not a canonical <provider>/<secret> secret name"))?;
    let is_segment = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    if !is_segment(provider) || !is_segment(secret) {
        bail!(
            "{name:?} is not a canonical <provider>/<secret> secret name: both \
             segments must be non-empty and lowercase [a-z0-9-]"
        );
    }
    Ok((provider, secret))
}

/// Read an environment variable, treating blank as unset.
fn non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_name_spells_itself_for_every_store() {
        // The property the whole scheme exists for: no store needs a mapping
        // table, and none of the three spellings can drift from the others.
        assert_eq!(env_var("oanda/api-key"), "OANDA_API_KEY");
        assert_eq!(aws_secret_id("oanda/api-key"), "dropset/oanda/api-key");
        assert_eq!(
            OnePasswordBackend::new("example-vault", None).reference("oanda/api-key"),
            "op://example-vault/oanda/api-key"
        );
    }

    #[test]
    fn a_multi_word_provider_keeps_one_spelling_per_store() {
        // `twelve-data` would fold to the same env var as `twelve/data`, which
        // is why the roster spells the provider as one word.
        assert_eq!(env_var("twelvedata/api-key"), "TWELVEDATA_API_KEY");
        assert_eq!(env_var("alphavantage/api-key"), "ALPHAVANTAGE_API_KEY");
    }

    #[test]
    fn a_name_that_would_break_a_1password_reference_is_rejected() {
        // Three segments would address a *section* in `op://vault/item/…`
        // rather than a field, and silently read the wrong thing.
        assert!(validate_name("dropset/oanda/api-key").is_err());
        // A dot is the earlier flattening scheme; the two are not compatible.
        assert!(validate_name("dropset.oanda.api-key").is_err());
        // One segment has no field to address.
        assert!(validate_name("oanda").is_err());
        assert!(validate_name("oanda/").is_err());
        assert!(validate_name("/api-key").is_err());
        // Case matters: `OANDA/API-KEY` would round-trip through `env_var`
        // unchanged and hide which spelling is canonical.
        assert!(validate_name("OANDA/api-key").is_err());
        assert!(validate_name("oanda/api key").is_err());

        assert_eq!(
            validate_name("oanda/api-key").unwrap(),
            ("oanda", "api-key")
        );
    }

    #[test]
    fn an_empty_environment_value_falls_through_rather_than_resolving() {
        // The motivating case: the compose services pass credentials as
        // `${VAR:-}`, so an unset key arrives as an empty string. Treating that
        // as a hit would hand the venue an empty key and turn a configuration
        // mistake into a puzzling 401 — and, once the enclave exists, would
        // shadow the vault that does hold the key.
        //
        // Scoped to a name nothing else uses: the process environment is shared
        // across the tests in a binary.
        let name = "env-probe/blank-case";
        std::env::set_var(env_var(name), "   ");
        assert!(EnvBackend.resolve(name).unwrap().is_none());

        std::env::set_var(env_var(name), "a-real-key");
        assert_eq!(
            EnvBackend.resolve(name).unwrap().as_deref(),
            Some("a-real-key")
        );
        std::env::remove_var(env_var(name));
    }

    /// A backend that reports a real one's naming but never holds anything, so
    /// the chain's fall-through and error text are exercised without running
    /// `op` against whatever vaults the machine happens to be signed in to.
    struct NeverHasIt(OnePasswordBackend);

    impl Backend for NeverHasIt {
        fn describe(&self) -> String {
            self.0.describe()
        }
        fn key_for(&self, name: &str) -> String {
            self.0.key_for(name)
        }
        fn resolve(&self, _name: &str) -> Result<Option<String>> {
            Ok(None)
        }
    }

    #[test]
    fn an_unresolvable_secret_names_every_key_it_looked_for() {
        // An operator who stored the key under the wrong name needs to see the
        // spellings that were actually tried, in both stores.
        let provider = SecretProvider::new(vec![
            Box::new(EnvBackend),
            Box::new(NeverHasIt(OnePasswordBackend::new("example-vault", None))),
        ]);
        let err = provider.resolve("no-such/api-key").unwrap_err().to_string();
        assert!(err.contains("NO_SUCH_API_KEY"), "{err}");
        assert!(err.contains("op://example-vault/no-such/api-key"), "{err}");
        assert!(err.contains("1Password vault \"example-vault\""), "{err}");
    }

    #[test]
    fn a_malformed_name_is_rejected_before_any_backend_runs() {
        // Validation ahead of the chain is what keeps a typo from reaching the
        // `op` subprocess as a half-formed reference.
        let provider = SecretProvider::new(vec![Box::new(OnePasswordBackend::new(
            "example-vault",
            None,
        ))]);
        assert!(provider.resolve("dropset.oanda.api-key").is_err());
    }

    #[test]
    fn the_environment_is_consulted_before_the_vault() {
        // The override path, and the reason CI needs no 1Password at all: a set
        // variable short-circuits the chain before any subprocess runs.
        let name = "env-probe/precedence-case";
        std::env::set_var(env_var(name), "from-the-environment");
        let provider = SecretProvider::new(vec![
            Box::new(EnvBackend),
            // Would fail loudly if it were ever consulted: no such vault.
            Box::new(OnePasswordBackend::new("no-such-vault", None)),
        ]);
        assert_eq!(provider.resolve(name).unwrap(), "from-the-environment");
        std::env::remove_var(env_var(name));
    }

    /// Resolve the FX roster against a **live** 1Password vault — the check
    /// that an operator's enclave is actually set up right, rather than that
    /// this module's logic is.
    ///
    /// `#[ignore]`d for the same reason the store tests are: it needs
    /// something no CI runner has — here `op`, signed in, with the vault
    /// populated. Run it after setting a machine up:
    ///
    /// ```sh
    /// DROPSET_OP_VAULT=<vault> cargo test -p dropset-feeds \
    ///   -- --ignored the_enclave
    /// ```
    ///
    /// It asserts only that each credential resolves to something non-empty,
    /// and never prints a value.
    #[test]
    #[ignore]
    #[cfg(feature = "http")]
    fn the_enclave_resolves_the_whole_fx_roster() {
        let vault = std::env::var(VAULT_ENV)
            .expect("set DROPSET_OP_VAULT to the vault to check, or skip this test");
        let provider = SecretProvider::new(vec![Box::new(OnePasswordBackend::new(
            vault,
            non_empty(ACCOUNT_ENV),
        ))]);
        for name in [
            crate::venues::oanda::SECRET_NAME,
            crate::venues::twelvedata::SECRET_NAME,
            crate::venues::alphavantage::SECRET_NAME,
        ] {
            let value = provider
                .resolve(name)
                .unwrap_or_else(|e| panic!("{name} did not resolve: {e}"));
            assert!(
                !value.trim().is_empty(),
                "{name} resolved to an empty value"
            );
        }
    }
}
