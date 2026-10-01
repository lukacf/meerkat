//! `Debug` views for secret-bearing configuration.
//!
//! Config types carrying env values, header values, command arguments or URLs
//! implement `Debug` by hand with these views, so a `{:?}` in a log line or
//! error never prints credentials. Names stay visible; values do not.

use std::fmt;

/// Marker printed in place of a secret value.
pub(crate) const REDACTED: &str = "<redacted>";

/// A secret-bearing map's keys in sorted order, each value shown as
/// [`REDACTED`].
pub(crate) struct RedactedValues<'a>(Vec<&'a str>);

impl<'a> RedactedValues<'a> {
    pub(crate) fn of(keys: impl IntoIterator<Item = &'a String>) -> Self {
        let mut keys: Vec<&str> = keys.into_iter().map(String::as_str).collect();
        keys.sort_unstable();
        Self(keys)
    }
}

impl fmt::Debug for RedactedValues<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.0.iter().map(|key| (key, REDACTED)))
            .finish()
    }
}

/// A secret-bearing list: its length, every value shown as [`REDACTED`].
pub(crate) struct RedactedList(pub(crate) usize);

impl fmt::Debug for RedactedList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(std::iter::repeat_n(REDACTED, self.0))
            .finish()
    }
}

/// A URL with scheme, host and path kept; userinfo, query and fragment
/// replaced by [`REDACTED`].
pub(crate) struct RedactedUrl<'a>(pub(crate) &'a str);

impl fmt::Debug for RedactedUrl<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (base, suffix) = match self.0.find(['?', '#']) {
            Some(index) => (&self.0[..index], Some(&self.0[index..=index])),
            None => (self.0, None),
        };
        let (scheme, rest) = match base.find("://") {
            Some(index) => base.split_at(index + 3),
            None => ("", base),
        };
        let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        let mut shown = String::with_capacity(self.0.len());
        shown.push_str(scheme);
        match authority.rfind('@') {
            Some(index) => {
                shown.push_str(REDACTED);
                shown.push_str(&authority[index..]);
            }
            None => shown.push_str(authority),
        }
        shown.push_str(path);
        if let Some(separator) = suffix {
            shown.push_str(separator);
            shown.push_str(REDACTED);
        }
        fmt::Debug::fmt(&shown, f)
    }
}

/// An optional secret: `None`, or `Some("<redacted>")`.
pub(crate) fn optional<T>(value: &Option<T>) -> Option<&'static str> {
    value.as_ref().map(|_| REDACTED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacted_url_keeps_location_and_hides_credentials() {
        for (url, shown) in [
            (
                "https://u:p@h.example/p?q=1",
                r#""https://<redacted>@h.example/p?<redacted>""#,
            ),
            (
                "http://127.0.0.1:8080/mcp",
                r#""http://127.0.0.1:8080/mcp""#,
            ),
            (
                "https://h.example/p#frag",
                r#""https://h.example/p#<redacted>""#,
            ),
            (
                "git@github.com:org/repo.git",
                r#""<redacted>@github.com:org/repo.git""#,
            ),
        ] {
            assert_eq!(format!("{:?}", RedactedUrl(url)), shown);
        }
    }
}
