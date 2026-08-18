//! The committed enclave template has to agree with the canonical secret names
//! the collectors actually ask for.
//!
//! The template (`infra/localnet/secrets.local.env.example`) is what an
//! operator copies to set a machine up, and `op run` resolves it into the
//! collector containers' environment. Nothing at compile time ties it to the
//! `SECRET_NAME` constants, so a renamed credential would leave a template that
//! still parses, still resolves, and exports the *old* variable — the
//! collector would then fail at startup on a machine that was correctly
//! configured. This test is the tie.

use dropset_feeds::{
    secrets::{env_var, validate_name},
    venues::{alphavantage, oanda, twelvedata},
};

/// The roster the FX collectors resolve — the same constants their binaries
/// pass to `secret()`.
const FX_SECRETS: [&str; 3] = [
    oanda::SECRET_NAME,
    twelvedata::SECRET_NAME,
    alphavantage::SECRET_NAME,
];

fn template() -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../infra/localnet/secrets.local.env.example"
    );
    std::fs::read_to_string(path)
        .expect("the enclave template is committed next to the compose file")
}

#[test]
fn the_template_carries_every_canonical_name_under_its_own_variable() {
    let template = template();
    for name in FX_SECRETS {
        validate_name(name).unwrap_or_else(|e| panic!("{name}: {e}"));
        // The variable is derived, never chosen: a line that paired the right
        // reference with a hand-written variable name would resolve into the
        // environment the collector does not read.
        let expected = format!("{}=op://<vault>/{name}", env_var(name));
        assert!(
            template.lines().any(|line| line.trim() == expected),
            "the template is missing `{expected}` — it and the SECRET_NAME \
             constants have drifted"
        );
    }
}

#[test]
fn the_template_names_no_real_vault_and_holds_no_value() {
    // The tracked copy is placeholders only: a real vault or item name here
    // would be a machine-local detail committed to the repo, and an `op://`
    // reference is the only shape a credential line may take.
    let template = template();
    for line in template.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .expect("a non-comment line is KEY=value");
        if key.ends_with("_API_KEY") {
            assert!(
                value.starts_with("op://<vault>/"),
                "{key} must be an op:// reference against the <vault> placeholder, got {value:?}"
            );
        } else {
            assert!(
                value.starts_with('<') || value.contains("<account>"),
                "{key} must be a placeholder in the tracked template, got {value:?}"
            );
        }
    }
}
