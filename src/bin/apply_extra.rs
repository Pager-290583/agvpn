//! Runs at Flatpak install time inside /app/extra: unpacks the downloaded adguardvpn-cli
//! archive (shipped as extra-data, because the CLI may not be redistributed).

use std::fs::File;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::ExitCode;

const ARCHIVE: &str = "adguardvpn-cli.tar.gz";
const WANTED: &[&str] = &["adguardvpn-cli", "adguardvpn-cli.sig", "bash-completion.sh"];

/// "adguardvpn-cli-1.7.12-linux-x86_64/..." -> "1.7.12"
fn version_from_path(path: &str) -> Option<String> {
    let rest = &path[path.find("adguardvpn-cli-")? + "adguardvpn-cli-".len()..];
    let version = &rest[..rest.find("-linux")?];
    let ok = version.chars().next().is_some_and(|c| c.is_ascii_digit())
        && version.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    ok.then(|| version.to_string())
}

fn run() -> std::io::Result<()> {
    let mut version = String::new();
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(File::open(ARCHIVE)?));
    for entry in tar.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?.to_string_lossy().into_owned();
        let name = path.rsplit('/').next().unwrap_or("").to_string();
        if !WANTED.contains(&name.as_str()) {
            continue;
        }
        if let Some(v) = version_from_path(&path) {
            version = v;
        }
        let mut out = File::create(&name)?;
        std::io::copy(&mut entry, &mut out)?;
        let mode = if name == "adguardvpn-cli.sig" { 0o644 } else { 0o755 };
        std::fs::set_permissions(&name, std::fs::Permissions::from_mode(mode))?;
    }
    if !std::path::Path::new("adguardvpn-cli").exists() {
        return Err(std::io::Error::other("adguardvpn-cli not found in the archive"));
    }
    File::create("VERSION")?.write_all(format!("{version}\n").as_bytes())?;
    std::fs::remove_file(ARCHIVE)
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("apply_extra: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn version() {
        assert_eq!(super::version_from_path("adguardvpn-cli-1.7.12-linux-x86_64/adguardvpn-cli").as_deref(), Some("1.7.12"));
        assert_eq!(super::version_from_path("x/adguardvpn-cli"), None);
    }
}
