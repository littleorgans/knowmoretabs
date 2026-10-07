//! The URL rules that decide what a network command may never request:
//! anything but the web, this machine and the private network, search
//! results, URLs that carry a secret, and sign-in or verification screens;
//! and the personal apps a signed in run never opens.
//!
//! slice: enrich, content
//! why: Every network command must refuse the same pages for the same
//!      reasons, before planning and again on every redirect hop, so the
//!      rules live in one place that both the planner and the fetcher call
//!      and that a reader can audit without reading any request code. They
//!      judge the URL alone; the resolver in `fetch` catches what a public
//!      name hides.

use std::net::IpAddr;

use url::{Host, Url};

use crate::local;

/// The host as one site: lowercase, without a trailing root dot.
pub fn host_key(url: &Url) -> String {
    url.host_str()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

pub fn is_web(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https") && url.host().is_some()
}

/// The address sent over HTTP, normalized and without its fragment. This
/// is also the identity used to refuse every variant of a forgotten page.
pub fn page_url(raw: &str) -> Option<Url> {
    let mut url = Url::parse(raw).ok()?;
    url.set_fragment(None);
    Some(url)
}

/// This machine or the private network, judged from the URL alone: an
/// address literal outside public space, a name that only means something
/// on a local network, or a bare name that a search domain would complete.
pub fn is_private_host(url: &Url) -> bool {
    match url.host() {
        None => true,
        Some(Host::Ipv4(ip)) => !is_public(IpAddr::V4(ip)),
        Some(Host::Ipv6(ip)) => !is_public(IpAddr::V6(ip)),
        Some(host @ Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            local::is_this_machine(&host)
                || !name.contains('.')
                || [
                    ".local",
                    ".localdomain",
                    ".internal",
                    ".intranet",
                    ".lan",
                    ".home",
                    ".home.arpa",
                    ".corp",
                ]
                .iter()
                .any(|suffix| name.ends_with(suffix))
        }
    }
}

/// Globally routable. Everything else is this machine, the private network
/// or a range nothing public lives in.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || a == 0
                || a >= 240
                || (a == 100 && (64..128).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 198 && (b == 18 || b == 19)))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let [first, second, ..] = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // Only native global unicast; exclude local, reserved and
                // transition ranges that can embed a private IPv4 target.
                || (first & 0xe000) != 0x2000
                || first == 0x2002
                || (first == 0x2001 && second == 0)
                || (first == 0x2001 && second == 0x0db8)
                || (first == 0x0064 && second == 0xff9b))
        }
    }
}

/// A results page says nothing the query in its URL does not.
pub fn is_search_results(url: &Url) -> bool {
    let host = host_key(url);
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let path = decoded_path(url).to_ascii_lowercase();
    let has = |key: &str| url.query_pairs().any(|(k, v)| k == key && !v.is_empty());
    let engine = |name: &str| {
        host == name || host.starts_with(&format!("{name}.")) || host.ends_with(&format!(".{name}"))
    };
    if engine("google") && ["/search", "/url", "/imgres", "/webhp"].contains(&path.as_str()) {
        return true;
    }
    if (engine("duckduckgo")
        && (has("q") || path.starts_with("/html") || path.starts_with("/lite")))
        || (engine("baidu") && path == "/s")
        || (engine("amazon") && path == "/s")
    {
        return true;
    }
    // Most sites put their own search at `/search` or `/results` with the
    // query in a parameter: Bing, Kagi, Brave, GitHub, YouTube, Reddit ...
    path.split('/')
        .any(|segment| matches!(segment.to_ascii_lowercase().as_str(), "search" | "results"))
        && url.query().is_some_and(|q| !q.is_empty())
}

/// Query parameter names that carry a secret or a one-time value. A GET can
/// spend a one-time link, and the value is nobody else's business anyway.
const TOKEN_WORDS: &[&str] = &[
    "token",
    "code",
    "key",
    "apikey",
    "accesskey",
    "authcode",
    "authorizationcode",
    "sessionkey",
    "resetkey",
    "secret",
    "sig",
    "signature",
    "auth",
    "session",
    "sessionid",
    "sid",
    "otp",
    "reset",
    "verify",
    "verification",
    "confirm",
    "confirmation",
    "magic",
    "nonce",
    "state",
    "password",
    "pwd",
    "ticket",
    "jwt",
    "credential",
    "credentials",
];

/// A query parameter whose name says it carries a secret, whose value is a
/// JSON Web Token, or credentials in the URL itself.
pub fn carries_token(url: &Url) -> bool {
    if !url.username().is_empty() || url.password().is_some() {
        return true;
    }
    url.query_pairs().any(|(key, value)| {
        let key = key.to_ascii_lowercase();
        let words_match = key
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| TOKEN_WORDS.contains(&word));
        words_match
            || ["token", "secret", "password", "signature"]
                .iter()
                .any(|word| key.contains(word))
            || (value.starts_with("eyJ") && value.len() > 30)
    })
}

/// Path segments that name a sign-in, sign-up or verification screen on
/// their own. A GET of a verification link can spend it, so these pages are
/// never fetched.
const LOGIN_SEGMENTS: &[&str] = &[
    "login",
    "log-in",
    "log_in",
    "logon",
    "signin",
    "sign-in",
    "sign_in",
    "signup",
    "sign-up",
    "sign_up",
    "register",
    "sso",
    "saml",
    "oauth",
    "oauth2",
    "authorize",
    "verify",
    "verify-email",
    "verify_email",
    "verification",
    "confirm",
    "confirmation",
    "activate",
    "activation",
    "reset",
    "confirm-email",
    "confirm_email",
    "reset-password",
    "reset_password",
    "password-reset",
    "password_reset",
    "forgot-password",
    "forgot_password",
    "magic-link",
    "magic_link",
    "2fa",
    "mfa",
    "otp",
    "servicelogin",
];
/// Weaker words that mean a login when a page redirects to them.
const REDIRECT_LOGIN_SEGMENTS: &[&str] = &["auth", "account", "accounts", "session", "sessions"];
const LOGIN_HOST_LABELS: &[&str] = &["login", "signin", "accounts", "auth", "sso", "idp"];

/// The owner's personal apps, each host with every subdomain of it: a
/// signed in run never opens a page on one, nor keeps one it ends on.
/// One line per kind; the consoles line is the owner's call.
#[rustfmt::skip]
const PERSONAL_APPS: &[&str] = &[
    // Mail
    "mail.google.com", "outlook.live.com", "outlook.office.com", "outlook.office365.com", "mail.yahoo.com", "mail.proton.me", "app.fastmail.com", "mail.zoho.com", "icloud.com",
    // Chat and assistants
    "claude.ai", "chatgpt.com", "chat.openai.com", "gemini.google.com", "aistudio.google.com", "grok.com", "meta.ai", "slack.com", "discord.com", "discordapp.com", "web.whatsapp.com", "web.telegram.org", "messenger.com", "teams.microsoft.com", "teams.live.com", "chat.google.com", "meet.google.com",
    // Documents and workspaces
    "docs.google.com", "drive.google.com", "calendar.google.com", "keep.google.com", "contacts.google.com", "photos.google.com", "notion.so", "notion.site", "onedrive.live.com", "sharepoint.com", "dropbox.com", "linear.app", "figma.com", "airtable.com", "trello.com", "atlassian.net",
    // Account consoles (owner question 2): delete this line to open them.
    "console.cloud.google.com", "console.tailscale.com", "console.instacloud.com", "console.aliyun.com", "cloud.databricks.com", "one.google.com", "platform.openai.com",
];

/// A page on one of the owner's personal apps.
pub fn is_personal_app(url: &Url) -> bool {
    let host = host_key(url);
    PERSONAL_APPS.iter().any(|app| {
        host.strip_suffix(app)
            .is_some_and(|rest| rest.is_empty() || rest.ends_with('.'))
    })
}

/// A sign-in, sign-up or verification screen in its own right.
pub fn is_login_page(url: &Url) -> bool {
    login_shaped(url, LOGIN_SEGMENTS)
}

/// A login screen, or a page that means one when a redirect lands on it.
pub fn is_login_redirect(url: &Url) -> bool {
    login_shaped(url, LOGIN_SEGMENTS) || login_shaped(url, REDIRECT_LOGIN_SEGMENTS)
}

fn decoded_path(url: &Url) -> std::borrow::Cow<'_, str> {
    percent_encoding::percent_decode_str(url.path()).decode_utf8_lossy()
}

fn login_shaped(url: &Url, words: &[&str]) -> bool {
    let host = host_key(url);
    let first_label = host.split('.').next().unwrap_or("");
    if host.contains('.') && LOGIN_HOST_LABELS.contains(&first_label) {
        return true;
    }
    decoded_path(url).split('/').any(|segment| {
        let segment = segment.to_ascii_lowercase();
        let stem = segment.split('.').next().unwrap_or("");
        words.contains(&stem)
    })
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use super::*;

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    #[test]
    fn private_hosts_are_known_from_the_url() {
        for raw in [
            "http://192.168.1.1/",
            "http://10.0.0.8:8080/",
            "http://172.16.4.4/",
            "http://100.64.0.1/",
            "http://169.254.169.254/latest/meta-data",
            "http://127.0.0.1/",
            "http://0.0.0.0/",
            "http://[::1]/",
            "http://[fd00::1]/",
            "http://[fec0::1]/",
            "http://[feff::1]/",
            "http://[2001::7f00:1]/",
            "http://[::127.0.0.1]/",
            "http://[2002:7f00:1::]/",
            "http://0.1.2.3/",
            "http://2130706433/",
            "http://0177.0.0.1/",
            "http://127.0.0.1./",
            "http://224.0.0.1/",
            "http://255.255.255.255/",
            "http://[ff02::1]/",
            "http://[::ffff:169.254.169.254]/",
            "http://[fe80::1]/",
            "http://[::ffff:192.168.0.1]/",
            "http://0x7f.1/",
            "http://printer.local/",
            "http://nas.home.arpa/",
            "http://build.internal/",
            "http://intranet/",
            "http://app.localhost/",
        ] {
            assert!(is_private_host(&url(raw)), "{raw}");
        }
        for raw in [
            "https://example.com/",
            "http://8.8.8.8/",
            "http://[2606:4700::1111]/",
            "https://local.example.com/",
            "https://localhost.example.test/",
        ] {
            assert!(!is_private_host(&url(raw)), "{raw}");
        }
        assert!(is_public(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
        assert!(!is_public(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn login_screens_are_known_by_their_own_url_and_redirects_by_more() {
        for raw in [
            "https://example.com/login",
            "https://example.com/users/sign_in",
            "https://example.com/accounts/login/?next=/x",
            "https://example.com/login.php",
            "https://example.com/oauth/authorize?client_id=x",
            "https://example.com/verify-email/abc",
            "https://accounts.example.com/ServiceLogin",
            "https://login.example.com/",
        ] {
            assert!(is_login_page(&url(raw)), "{raw}");
        }
        for raw in [
            "https://example.com/blog/how-login-works",
            "https://github.com/owner/auth",
            "https://example.com/account",
            "https://login/",
        ] {
            assert!(!is_login_page(&url(raw)), "{raw}");
        }
        assert!(is_login_redirect(&url(
            "https://example.com/account?return=/"
        )));
        assert!(is_login_redirect(&url("https://example.com/auth/start")));
        assert!(!is_login_redirect(&url("https://example.com/docs/")));
    }

    #[test]
    fn personal_apps_are_known_by_host_and_subdomain_and_never_by_lookalike() {
        for raw in [
            "https://mail.google.com/mail/u/0/#inbox",
            "https://claude.ai/chat/x",
            "https://www.meta.ai/",
            "https://NOTION.SO./page",
            "https://team.notion.so/page",
            "https://acme.atlassian.net/browse/X-1",
            "https://smartservice.console.aliyun.com/",
        ] {
            assert!(is_personal_app(&url(raw)), "{raw}");
        }
        assert!(
            is_personal_app(&url("https://console.tailscale.com/admin")),
            "consoles (owner question 2)"
        );
        for raw in [
            "https://xnotion.so/",
            "https://notion.so.evil.test/",
            "https://mail.google.com.evil.test/",
            "https://google.com/",
            "https://docs.github.com/",
            "https://page-intl.aliyun.com/",
            "https://github.com/owner/repo",
        ] {
            assert!(!is_personal_app(&url(raw)), "{raw}");
        }
    }

    #[test]
    fn search_results_pages_are_known_by_engine_and_by_shape() {
        for raw in [
            "https://www.google.com/search?q=rust",
            "https://www.google.co.uk/search?q=rust&tbm=isch",
            "https://www.google.com/url?q=https://example.com",
            "https://duckduckgo.com/?q=rust",
            "https://www.bing.com/search?q=rust",
            "https://kagi.com/search?q=rust",
            "https://search.brave.com/search?q=rust",
            "https://github.com/search?q=parser&type=repositories",
            "https://www.youtube.com/results?search_query=rust",
            "https://www.baidu.com/s?wd=rust",
            "https://www.amazon.co.uk/s?k=kettle",
        ] {
            assert!(is_search_results(&url(raw)), "{raw}");
        }
        for raw in [
            "https://www.google.com/maps/place/x",
            "https://duckduckgo.com/about",
            "https://example.com/search",
            "https://example.com/research?q=x",
            "https://example.com/blog/how-search-works",
        ] {
            assert!(!is_search_results(&url(raw)), "{raw}");
        }
    }

    #[test]
    fn token_parameters_are_known_by_name_value_and_credentials() {
        for raw in [
            "https://example.com/a?token=abc",
            "https://example.com/a?access_token=abc",
            "https://example.com/a?X-Amz-Signature=abc",
            "https://example.com/cb?code=abc&state=xyz",
            "https://example.com/a?resetPasswordToken=1",
            "https://example.com/a?api-key=1",
            "https://example.com/a?t=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.sig",
            "https://user:pass@example.com/",
        ] {
            assert!(carries_token(&url(raw)), "{raw}");
        }
        for raw in [
            "https://example.com/a?author=bob",
            "https://example.com/a?keywords=rust&monkey=1",
            "https://example.com/a?page=2&sort=new",
            "https://example.com/a?estate=house&design=x",
            "https://example.com/zip?postcode=AB1",
        ] {
            assert!(!carries_token(&url(raw)), "{raw}");
        }
    }
}
