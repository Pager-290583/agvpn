//! Downloads newer adguardvpn-cli releases from GitHub into the app data dir.
//!
//! The Flatpak ships the CLI as extra-data; this lets the app follow the CLI's own update
//! channel (release/beta/nightly) without waiting for a new Flatpak build.

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use regex::Regex;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::cli::downloaded_cli_dir;
use crate::consts::{CLI_NAME, CLI_REPO};
use crate::parsers::version_tuple;
use crate::util::set_mode;

#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Deserialize)]
struct GhRelease {
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    #[serde(default)]
    name: String,
    #[serde(default)]
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
}

pub fn cli_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        "arm" => "armv7",
        other => other,
    }
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(timeout)).build().into()
}

pub fn latest_release(channel: &str) -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{CLI_REPO}/releases?per_page=40");
    let releases: Vec<GhRelease> = agent(Duration::from_secs(30))
        .get(&url)
        .header("User-Agent", "AGVPN")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_json()
        .map_err(|e| e.to_string())?;
    let pattern = Regex::new(&format!(
        r"^{}-(.+)-linux-{}\.tar\.gz$",
        regex::escape(CLI_NAME),
        regex::escape(cli_arch())
    ))
    .unwrap();
    let mut best: Option<Release> = None;
    for rel in releases {
        if rel.draft || !rel.tag_name.ends_with(&format!("-{channel}")) {
            continue;
        }
        for asset in rel.assets {
            let Some(caps) = pattern.captures(&asset.name) else { continue };
            let Some(sha) = asset.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")) else { continue };
            let cand = Release { version: caps[1].to_string(), url: asset.browser_download_url, sha256: sha.to_string() };
            if best.as_ref().is_none_or(|b| version_tuple(&cand.version) > version_tuple(&b.version)) {
                best = Some(cand);
            }
        }
    }
    Ok(best)
}

/// Downloads, verifies and unpacks the release. Returns the path of the binary.
pub fn install_release(rel: &Release) -> Result<String, String> {
    let root = downloaded_cli_dir();
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let mut archive = Vec::new();
    agent(Duration::from_secs(300))
        .get(&rel.url)
        .header("User-Agent", "AGVPN")
        .call()
        .map_err(|e| e.to_string())?
        .into_body()
        .into_reader()
        .read_to_end(&mut archive)
        .map_err(|e| e.to_string())?;
    let digest: String = Sha256::digest(&archive).iter().map(|b| format!("{b:02x}")).collect();
    if digest != rel.sha256.to_lowercase() {
        return Err("Checksum mismatch for the downloaded adguardvpn-cli archive".into());
    }
    let target_dir = root.join(&rel.version);
    std::fs::create_dir_all(&target_dir).map_err(|e| e.to_string())?;
    let target = target_dir.join(CLI_NAME);
    if !extract_member(&archive, CLI_NAME, &target).map_err(|e| e.to_string())? {
        return Err("adguardvpn-cli not found in the archive".into());
    }
    cleanup_old(&root, &rel.version);
    Ok(target.to_string_lossy().into_owned())
}

/// Extracts the regular file named `name` (at any depth) from a .tar.gz into `target`.
pub fn extract_member(archive: &[u8], name: &str, target: &Path) -> std::io::Result<bool> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?.into_owned();
        if path.file_name().and_then(|n| n.to_str()) != Some(name) {
            continue;
        }
        let tmp = target.with_extension("tmp");
        let mut out = std::fs::File::create(&tmp)?;
        std::io::copy(&mut entry, &mut out)?;
        drop(out);
        set_mode(&tmp, 0o755);
        std::fs::rename(&tmp, target)?;
        return Ok(true);
    }
    Ok(false)
}

fn cleanup_old(root: &Path, keep: &str) {
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            if entry.file_name() != keep {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}
