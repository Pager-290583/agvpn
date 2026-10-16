//! Static reference data: popular DNS servers and website groups for exclusions.

pub fn flag_emoji(iso: &str) -> String {
    let iso = iso.to_uppercase();
    let bytes = iso.as_bytes();
    if bytes.len() != 2 || !bytes.iter().all(u8::is_ascii_alphabetic) {
        return "🌐".into();
    }
    bytes.iter().filter_map(|b| char::from_u32(0x1F1E6 + u32::from(b - b'A'))).collect()
}

pub struct DnsPreset {
    pub name: &'static str,
    pub description: &'static str,
    /// Anything `config set-dns` accepts: IP, tls://, https://, quic://, sdns://
    pub address: &'static str,
}

pub static DNS_PRESETS: &[DnsPreset] = &[
    DnsPreset { name: "AdGuard DNS", description: "Blocks ads, trackers and phishing", address: "https://dns.adguard-dns.com/dns-query" },
    DnsPreset { name: "AdGuard DNS Non-filtering", description: "Doesn't block anything", address: "https://unfiltered.adguard-dns.com/dns-query" },
    DnsPreset { name: "AdGuard DNS Family Protection", description: "Blocks ads, trackers, adult content, enables Safe Search", address: "https://family.adguard-dns.com/dns-query" },
    DnsPreset { name: "Google Public DNS", description: "Doesn't block anything", address: "https://dns.google/dns-query" },
    DnsPreset { name: "Cloudflare DNS", description: "Doesn't block anything", address: "https://cloudflare-dns.com/dns-query" },
    DnsPreset { name: "Cloudflare for Families", description: "Blocks malware and adult content", address: "https://family.cloudflare-dns.com/dns-query" },
    DnsPreset { name: "Quad9", description: "Blocks malicious domains", address: "https://dns.quad9.net/dns-query" },
    DnsPreset { name: "Cisco OpenDNS", description: "Blocks phishing", address: "https://doh.opendns.com/dns-query" },
    DnsPreset { name: "CleanBrowsing Family", description: "Blocks adult content", address: "https://doh.cleanbrowsing.org/doh/family-filter/" },
    DnsPreset { name: "Mullvad DNS", description: "Doesn't block anything", address: "https://dns.mullvad.net/dns-query" },
];

pub struct Service {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub domains: &'static [&'static str],
}

// Website groups offered in "Add website → Services". Each domain is added together with
// its subdomains (`example.com` + `*.example.com`), as the macOS app does.
pub static SERVICES: &[Service] = &[
    Service { id: "youtube", name: "YouTube", category: "Streaming", domains: &["youtube.com", "youtu.be", "ytimg.com", "googlevideo.com", "youtube-nocookie.com", "youtubei.googleapis.com", "yt3.ggpht.com"] },
    Service { id: "netflix", name: "Netflix", category: "Streaming", domains: &["netflix.com", "netflix.net", "nflxext.com", "nflximg.com", "nflximg.net", "nflxso.net", "nflxvideo.net"] },
    Service { id: "twitch", name: "Twitch", category: "Streaming", domains: &["twitch.tv", "ttvnw.net", "jtvnw.net", "twitchcdn.net", "twitchsvc.net"] },
    Service { id: "spotify", name: "Spotify", category: "Streaming", domains: &["spotify.com", "scdn.co", "spotifycdn.com", "spotify.design", "pscdn.co"] },
    Service { id: "disneyplus", name: "Disney+", category: "Streaming", domains: &["disneyplus.com", "disney-plus.net", "bamgrid.com", "dssott.com"] },
    Service { id: "primevideo", name: "Prime Video", category: "Streaming", domains: &["primevideo.com", "aiv-cdn.net", "aiv-delivery.net", "amazonvideo.com"] },
    Service { id: "hulu", name: "Hulu", category: "Streaming", domains: &["hulu.com", "hulustream.com", "huluim.com"] },
    Service { id: "kinopoisk", name: "Kinopoisk", category: "Streaming", domains: &["kinopoisk.ru", "kinopoisk.tv"] },
    Service { id: "facebook", name: "Facebook", category: "Social networks", domains: &["facebook.com", "fb.com", "fbcdn.net", "facebook.net", "fb.me", "messenger.com"] },
    Service { id: "instagram", name: "Instagram", category: "Social networks", domains: &["instagram.com", "cdninstagram.com", "ig.me"] },
    Service { id: "twitter", name: "X (Twitter)", category: "Social networks", domains: &["twitter.com", "x.com", "twimg.com", "t.co"] },
    Service { id: "tiktok", name: "TikTok", category: "Social networks", domains: &["tiktok.com", "tiktokcdn.com", "tiktokv.com", "byteoversea.com", "ibytedtos.com", "musical.ly"] },
    Service { id: "reddit", name: "Reddit", category: "Social networks", domains: &["reddit.com", "redd.it", "redditmedia.com", "redditstatic.com"] },
    Service { id: "linkedin", name: "LinkedIn", category: "Social networks", domains: &["linkedin.com", "licdn.com", "lnkd.in"] },
    Service { id: "pinterest", name: "Pinterest", category: "Social networks", domains: &["pinterest.com", "pinimg.com"] },
    Service { id: "vk", name: "VK", category: "Social networks", domains: &["vk.com", "vk.ru", "userapi.com", "vk-cdn.net", "vkuser.net", "vk.me"] },
    Service { id: "ok", name: "Odnoklassniki", category: "Social networks", domains: &["ok.ru", "mycdn.me", "odnoklassniki.ru"] },
    Service { id: "telegram", name: "Telegram", category: "Messengers", domains: &["telegram.org", "t.me", "telegram.me", "telesco.pe", "tdesktop.com"] },
    Service { id: "whatsapp", name: "WhatsApp", category: "Messengers", domains: &["whatsapp.com", "whatsapp.net", "wa.me"] },
    Service { id: "discord", name: "Discord", category: "Messengers", domains: &["discord.com", "discord.gg", "discordapp.com", "discordapp.net", "discord.media"] },
    Service { id: "signal", name: "Signal", category: "Messengers", domains: &["signal.org", "whispersystems.org", "signal.art"] },
    Service { id: "zoom", name: "Zoom", category: "Messengers", domains: &["zoom.us", "zoom.com", "zoomgov.com"] },
    Service { id: "skype", name: "Skype", category: "Messengers", domains: &["skype.com", "skypeassets.com", "lync.com"] },
    Service { id: "google", name: "Google", category: "Search & portals", domains: &["google.com", "gstatic.com", "googleapis.com", "googleusercontent.com", "gmail.com", "google.ru"] },
    Service { id: "yandex", name: "Yandex", category: "Search & portals", domains: &["yandex.ru", "yandex.net", "yandex.com", "ya.ru", "yastatic.net", "yandex.by", "yandex.kz"] },
    Service { id: "mailru", name: "Mail.ru", category: "Search & portals", domains: &["mail.ru", "imgsmail.ru", "mradx.net"] },
    Service { id: "bing", name: "Microsoft Bing", category: "Search & portals", domains: &["bing.com", "bing.net"] },
    Service { id: "wikipedia", name: "Wikipedia", category: "Search & portals", domains: &["wikipedia.org", "wikimedia.org", "wikidata.org"] },
    Service { id: "chatgpt", name: "ChatGPT", category: "AI", domains: &["chatgpt.com", "openai.com", "oaistatic.com", "oaiusercontent.com"] },
    Service { id: "claude", name: "Claude", category: "AI", domains: &["claude.ai", "anthropic.com", "claudeusercontent.com"] },
    Service { id: "gemini", name: "Gemini", category: "AI", domains: &["gemini.google.com", "aistudio.google.com"] },
    Service { id: "amazon", name: "Amazon", category: "Shopping", domains: &["amazon.com", "amazon.de", "amazon.co.uk", "media-amazon.com", "ssl-images-amazon.com"] },
    Service { id: "aliexpress", name: "AliExpress", category: "Shopping", domains: &["aliexpress.com", "aliexpress.ru", "alicdn.com"] },
    Service { id: "ebay", name: "eBay", category: "Shopping", domains: &["ebay.com", "ebaystatic.com", "ebayimg.com"] },
    Service { id: "ozon", name: "Ozon", category: "Shopping", domains: &["ozon.ru", "ozone.ru", "ozonusercontent.com"] },
    Service { id: "wildberries", name: "Wildberries", category: "Shopping", domains: &["wildberries.ru", "wb.ru", "wbbasket.ru"] },
    Service { id: "steam", name: "Steam", category: "Games", domains: &["steampowered.com", "steamcommunity.com", "steamstatic.com", "steamcontent.com", "steamserver.net"] },
    Service { id: "epic", name: "Epic Games", category: "Games", domains: &["epicgames.com", "unrealengine.com", "epicgames.dev"] },
    Service { id: "playstation", name: "PlayStation", category: "Games", domains: &["playstation.com", "playstation.net", "sonyentertainmentnetwork.com"] },
    Service { id: "xbox", name: "Xbox", category: "Games", domains: &["xbox.com", "xboxlive.com", "xboxservices.com"] },
    Service { id: "github", name: "GitHub", category: "Work", domains: &["github.com", "githubusercontent.com", "githubassets.com", "github.io"] },
    Service { id: "microsoft", name: "Microsoft 365", category: "Work", domains: &["microsoft.com", "office.com", "office.net", "live.com", "outlook.com", "microsoftonline.com", "sharepoint.com"] },
    Service { id: "apple", name: "Apple", category: "Work", domains: &["apple.com", "icloud.com", "mzstatic.com", "apple-cloudkit.com", "cdn-apple.com"] },
    Service { id: "dropbox", name: "Dropbox", category: "Work", domains: &["dropbox.com", "dropboxapi.com", "dropboxusercontent.com", "db.tt"] },
    Service { id: "slack", name: "Slack", category: "Work", domains: &["slack.com", "slack-edge.com", "slack-imgs.com", "slack-msgs.com"] },
    Service { id: "notion", name: "Notion", category: "Work", domains: &["notion.so", "notion.com", "notion.site"] },
    Service { id: "gosuslugi", name: "Gosuslugi", category: "Government & banks", domains: &["gosuslugi.ru", "esia.gosuslugi.ru"] },
    Service { id: "nalog", name: "FNS (nalog.ru)", category: "Government & banks", domains: &["nalog.ru", "nalog.gov.ru"] },
    Service { id: "sber", name: "Sber", category: "Government & banks", domains: &["sberbank.ru", "sber.ru", "sberbank.com", "online.sberbank.ru"] },
    Service { id: "tbank", name: "T-Bank", category: "Government & banks", domains: &["tbank.ru", "tinkoff.ru", "tinkoff.com", "tcsbank.ru"] },
    Service { id: "vtb", name: "VTB", category: "Government & banks", domains: &["vtb.ru", "online.vtb.ru"] },
    Service { id: "alfabank", name: "Alfa-Bank", category: "Government & banks", domains: &["alfabank.ru", "alfa-bank.ru"] },
    Service { id: "paypal", name: "PayPal", category: "Government & banks", domains: &["paypal.com", "paypalobjects.com", "paypal.me"] },
];

pub fn service_patterns(domains: &[&str]) -> Vec<String> {
    domains.iter().flat_map(|d| [d.to_string(), format!("*.{d}")]).collect()
}

pub fn service_by_id(id: &str) -> Option<&'static Service> {
    SERVICES.iter().find(|s| s.id == id)
}

#[cfg(test)]
mod tests {
    #[test]
    fn flags() {
        assert_eq!(super::flag_emoji("de"), "🇩🇪");
        assert_eq!(super::flag_emoji(""), "🌐");
    }
}
