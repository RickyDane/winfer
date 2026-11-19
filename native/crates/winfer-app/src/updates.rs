//! App updates and release notes (port of `features/updates/releaseNotes.ts` and the checking half
//! of `updater.ts`). The native build checks GitHub releases tagged `native-v<version>` (Nebula
//! builds carry a numeric pre-release, `native-v0.2.0-7`) and points at the platform's download; it
//! doesn't replace itself, since the build isn't signed for in-place updates yet.

use serde::Deserialize;

pub const REPO: &str = "RickyDane/winfer";
pub const TAG_PREFIX: &str = "native-v";

/// One release's changes, as the "What's new" dialog lists them.
#[derive(Clone, Debug, PartialEq)]
pub struct ReleaseChanges {
    pub version: String,
    pub changes: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Nebula,
}

impl Channel {
    pub fn id(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Nebula => "nebula",
        }
    }

    pub fn parse(id: &str) -> Option<Channel> {
        match id {
            "stable" => Some(Channel::Stable),
            "nebula" => Some(Channel::Nebula),
            _ => None,
        }
    }

    /// The channel of a build, from its version (Nebula builds are versioned X.Y.Z-N).
    pub fn of_version(version: &str) -> Channel {
        if version.contains('-') { Channel::Nebula } else { Channel::Stable }
    }
}

/// Release notes as the release workflow writes them: a "### Changes" list of commit subjects, then
/// a "---" and the install footer. Only the changes are worth showing in the app.
pub fn parse_changes(body: Option<&str>) -> Vec<String> {
    let Some(body) = body else { return Vec::new() };
    let lines: Vec<&str> = body.lines().collect();
    let start = lines.iter().position(|l| {
        let t = l.trim();
        t.starts_with('#') && t.trim_start_matches('#').trim().eq_ignore_ascii_case("changes")
    });
    let mut changes = Vec::new();
    for line in lines.iter().skip(start.map_or(0, |s| s + 1)) {
        let t = line.trim();
        if t == "---" || (start.is_some() && t.starts_with('#')) {
            break;
        }
        if let Some(item) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            if !item.trim().is_empty() {
                changes.push(item.trim().to_string());
            }
        }
    }
    changes
}

/// "Plugins: add Telegram" → (Some("Plugins"), "add Telegram"); no prefix → no area.
pub fn split_area(change: &str) -> (Option<String>, String) {
    if let Some((head, rest)) = change.split_once(": ") {
        let mut chars = head.chars();
        let ok = head.len() <= 31 && chars.next().is_some_and(|c| c.is_ascii_uppercase()) && chars.all(|c| c.is_alphanumeric() || " ./+-_".contains(c)) && !rest.trim().is_empty();
        if ok {
            return (Some(head.to_string()), rest.trim().to_string());
        }
    }
    (None, change.to_string())
}

fn parts(v: &str) -> (Vec<u64>, Option<Vec<u64>>) {
    let v = v.trim_start_matches('v');
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (v, None),
    };
    let nums = |s: &str| s.split('.').map(|p| p.parse().unwrap_or(0)).collect::<Vec<u64>>();
    (nums(core), pre.map(nums))
}

fn cmp_nums(a: &[u64], b: &[u64]) -> std::cmp::Ordering {
    for i in 0..a.len().max(b.len()) {
        let o = a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0));
        if o != std::cmp::Ordering::Equal {
            return o;
        }
    }
    std::cmp::Ordering::Equal
}

/// Orders "0.1.9" before "0.1.10"; a leading "v" is ignored. A numeric pre-release sorts before its release.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let (ca, pa) = parts(a);
    let (cb, pb) = parts(b);
    let o = cmp_nums(&ca, &cb);
    if o != std::cmp::Ordering::Equal {
        return o;
    }
    match (pa, pb) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(x), Some(y)) => cmp_nums(&x, &y),
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

impl Release {
    /// The version a native release carries, or None for other releases in the repo (the Tauri app's).
    pub fn version(&self) -> Option<String> {
        self.tag_name.strip_prefix(TAG_PREFIX).map(str::to_string)
    }
}

/// The releases after `from` up to and including `to`, newest first, so skipping versions still
/// shows everything that changed. No `from` → just `to`. Nebula (pre-release) builds only count
/// when updating to one; a stable release's notes already cover everything since the last stable.
pub fn releases_between(releases: &[Release], from: Option<&str>, to: &str) -> Vec<ReleaseChanges> {
    use std::cmp::Ordering::*;
    let nebula = to.contains('-');
    let mut out: Vec<ReleaseChanges> = releases
        .iter()
        .filter(|r| !r.draft && (nebula || !r.prerelease))
        .filter_map(|r| Some(ReleaseChanges { version: r.version()?, changes: parse_changes(r.body.as_deref()) }))
        .filter(|r| compare_versions(&r.version, to) != Greater && match from {
            Some(f) => compare_versions(&r.version, f) == Greater,
            None => compare_versions(&r.version, to) == Equal,
        })
        .collect();
    out.sort_by(|a, b| compare_versions(&b.version, &a.version));
    out
}

/// The newest release on `channel` newer than `current`.
pub fn newer<'a>(releases: &'a [Release], current: &str, channel: Channel) -> Option<&'a Release> {
    releases
        .iter()
        .filter(|r| !r.draft && (channel == Channel::Nebula || !r.prerelease))
        .filter_map(|r| Some((r.version()?, r)))
        .filter(|(v, _)| compare_versions(v, current) == std::cmp::Ordering::Greater)
        .max_by(|(a, _), (b, _)| compare_versions(a, b))
        .map(|(_, r)| r)
}

/// The download for this platform among a release's assets, else the release page.
pub fn download_url(release: &Release) -> String {
    let (os, arch): (&[&str], &[&str]) = if cfg!(target_os = "macos") {
        (&["macos", "darwin", "mac", ".dmg"], if cfg!(target_arch = "aarch64") { &["arm64", "aarch64"] } else { &["x86_64", "x64", "amd64", "intel"] })
    } else if cfg!(target_os = "windows") {
        (&["windows", "win", ".msi", ".exe"], &["x86_64", "x64", "amd64"])
    } else {
        (&["linux", ".appimage", ".deb"], if cfg!(target_arch = "aarch64") { &["arm64", "aarch64"] } else { &["x86_64", "x64", "amd64"] })
    };
    let lower = |a: &Asset| a.name.to_lowercase();
    let for_os = |a: &&Asset| os.iter().any(|k| lower(a).contains(k));
    let matches: Vec<&Asset> = release.assets.iter().filter(for_os).collect();
    matches.iter().find(|a| arch.iter().any(|k| lower(a).contains(k))).or(matches.first()).map_or_else(|| release.html_url.clone(), |a| a.browser_download_url.clone())
}

/// All releases of the repo (blocking: call off the UI thread).
pub fn fetch_releases() -> Result<Vec<Release>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(15))).build().into();
    let mut res = agent
        .get(&format!("https://api.github.com/repos/{REPO}/releases?per_page=100"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "winfer")
        .call()
        .map_err(|e| format!("GitHub answered: {e}"))?;
    res.body_mut().read_json::<Vec<Release>>().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    const BODY: &str = "### Changes\n- Plugins: add Telegram, answer agents from your phone\n- Settings: start agents in auto-approve mode\n\n---\n### Install\nUnsigned builds.\n\n- not a change\n";

    fn rel(tag: &str, body: &str) -> Release {
        Release { tag_name: format!("{TAG_PREFIX}{tag}"), body: Some(body.into()), draft: false, prerelease: tag.contains('-'), html_url: String::new(), assets: Vec::new() }
    }

    #[test]
    fn reads_the_changes_list_and_stops_at_the_footer() {
        assert_eq!(parse_changes(Some(BODY)), ["Plugins: add Telegram, answer agents from your phone", "Settings: start agents in auto-approve mode"]);
        assert_eq!(parse_changes(Some("- one\n* two\n---\n- three")), ["one", "two"]);
        assert!(parse_changes(None).is_empty() && parse_changes(Some("")).is_empty());
    }

    #[test]
    fn splits_the_area() {
        assert_eq!(split_area("Browser and side terminal: keep tabs"), (Some("Browser and side terminal".into()), "keep tabs".into()));
        assert_eq!(split_area("fix crash on start"), (None, "fix crash on start".into()));
        assert_eq!(split_area("see: this"), (None, "see: this".into()));
    }

    #[test]
    fn compares_versions() {
        assert_eq!(compare_versions("0.1.9", "0.1.10"), Ordering::Less);
        assert_eq!(compare_versions("v0.2.0", "0.1.43"), Ordering::Greater);
        assert_eq!(compare_versions("0.1.43", "v0.1.43"), Ordering::Equal);
        assert_eq!(compare_versions("0.1.102-7", "0.1.102"), Ordering::Less);
        assert_eq!(compare_versions("0.1.102-7", "0.1.101"), Ordering::Greater);
        assert_eq!(compare_versions("0.1.102-9", "0.1.102-10"), Ordering::Less);
    }

    #[test]
    fn lists_what_changed_between_versions() {
        let releases = vec![rel("0.1.44", "- four"), rel("0.1.43", "- three"), rel("0.1.42", "- two"), rel("0.1.41", "- one"), Release { draft: true, ..rel("0.1.45", "- draft") }, Release { tag_name: "v9.9.9".into(), ..rel("0", "- tauri") }];
        let got = releases_between(&releases, Some("0.1.41"), "0.1.43");
        assert_eq!(got, [ReleaseChanges { version: "0.1.43".into(), changes: vec!["three".into()] }, ReleaseChanges { version: "0.1.42".into(), changes: vec!["two".into()] }]);
        assert_eq!(releases_between(&releases, None, "0.1.44"), [ReleaseChanges { version: "0.1.44".into(), changes: vec!["four".into()] }]);
        let mut with_nebula = releases.clone();
        with_nebula.push(rel("0.1.45-3", "- u3"));
        with_nebula.push(rel("0.1.45-2", "- u2"));
        assert_eq!(releases_between(&with_nebula, Some("0.1.44"), "0.1.45-3").iter().map(|r| r.version.as_str()).collect::<Vec<_>>(), ["0.1.45-3", "0.1.45-2"]);
        assert_eq!(releases_between(&with_nebula, Some("0.1.43"), "0.1.44").len(), 1);
    }

    #[test]
    fn finds_the_newer_release_on_a_channel() {
        let releases = vec![rel("0.1.44", ""), rel("0.1.45-2", ""), rel("0.1.43", "")];
        assert_eq!(newer(&releases, "0.1.43", Channel::Stable).and_then(Release::version).as_deref(), Some("0.1.44"));
        assert_eq!(newer(&releases, "0.1.43", Channel::Nebula).and_then(Release::version).as_deref(), Some("0.1.45-2"));
        assert!(newer(&releases, "0.1.44", Channel::Stable).is_none());
        assert_eq!(Channel::of_version("0.1.102-7"), Channel::Nebula);
    }

    #[test]
    fn picks_this_platforms_download() {
        let mut r = rel("0.2.0", "");
        r.html_url = "https://example/release".into();
        assert_eq!(download_url(&r), "https://example/release");
        let asset = |n: &str| Asset { name: n.into(), browser_download_url: format!("https://example/{n}") };
        r.assets = vec![asset("winfer-linux-x86_64.AppImage"), asset("winfer-macos-arm64.dmg"), asset("winfer-macos-x86_64.dmg"), asset("winfer-windows-x64.msi")];
        let url = download_url(&r);
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            assert_eq!(url, "https://example/winfer-macos-arm64.dmg");
        }
        assert!(url.starts_with("https://example/"));
    }
}
