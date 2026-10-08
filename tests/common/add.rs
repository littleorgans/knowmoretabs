//! What the `add` tests share: a page worth keeping, the binary run with
//! every name resolved to the local site, and the lines it leaves.

use std::process::Output;

use serde_json::Value;

use super::Fixture;
use super::site::Site;

/// An article with enough text to keep, and no image.
pub fn article(title: &str) -> String {
    format!(
        "<html><head><title>{title}</title></head><body><main><h1>{title}</h1><p>{}</p></main></body></html>",
        "Words the page says about itself. ".repeat(150)
    )
}

/// The binary with `args` and every name resolved to `site`.
pub fn at_site(fx: &Fixture, site: &Site, args: &[&str]) -> Output {
    fx.command()
        .args(args)
        .env("KNOWMORETABS_TEST_RESOLVE", site.address.to_string())
        .env("KNOWMORETABS_TEST_TIMEOUT_MS", "400")
        .output()
        .unwrap()
}

/// `add` with every name resolved to `site`.
pub fn add(fx: &Fixture, site: &Site, args: &[&str]) -> Output {
    let mut all = vec!["add"];
    all.extend_from_slice(args);
    at_site(fx, site, &all)
}

/// The intake log's lines.
pub fn intake(fx: &Fixture) -> Vec<Value> {
    std::fs::read_to_string(fx.root.join("pages").join("added.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The JSON lines on stdout.
pub fn lines(output: &Output) -> Vec<Value> {
    super::stdout(output)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
