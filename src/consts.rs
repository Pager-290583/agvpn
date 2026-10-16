//! Application-wide constants.

pub const APP_ID: &str = "io.github.agvpn.AGVPN";
pub const APP_NAME: &str = "AGVPN";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const CLI_NAME: &str = "adguardvpn-cli";
pub const CLI_REPO: &str = "AdguardTeam/AdGuardVPNCLI";

// Where the optional passwordless helper is installed on the host.
pub const HOST_HELPER: &str = "/usr/local/libexec/agvpn/agvpn-helper";
pub const HOST_HELPER_CLI: &str = "/usr/local/libexec/agvpn/adguardvpn-cli";

pub const URL_WEBSITE: &str = "https://adguard-vpn.com/";
pub const URL_EULA: &str = "https://adguard-vpn.com/eula.html";
pub const URL_PRIVACY: &str = "https://adguard-vpn.com/privacy.html";
pub const URL_FAQ: &str = "https://adguard-vpn.com/kb/adguard-vpn-for-linux/overview/";
pub const URL_ACCOUNT: &str = "https://adguardaccount.com/account/";
pub const URL_UPGRADE: &str = "https://agrd.io/vpn_cli";
pub const URL_FEEDBACK: &str = "https://github.com/AdguardTeam/AdGuardVPNCLI/discussions";
pub const URL_REPORT_BUG: &str = "https://github.com/AdguardTeam/AdGuardVPNCLI/issues/new";
pub const URL_FORGOT_PASSWORD: &str = "https://adguardaccount.com/recovery_password.html";
pub const SUPPORT_EMAIL: &str = "support@adguard-vpn.com";
