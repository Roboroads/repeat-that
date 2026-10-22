//! What the window shows about a release: its notes as plain text, the page that has the full notes, and whether the
//! version is newer than the running one. The release information comes from a file on GitHub, so none of it is taken
//! as is.

use crate::commands::LINKS;

/// The most characters of release notes the window gets.
pub const NOTES_MAX_CHARS: usize = 4000;

/// The notes as the window shows them: plain text with no HTML comments (release bodies carry markers like
/// `<!-- macos-untested -->`), no control characters, at most [`NOTES_MAX_CHARS`] characters.
pub fn clean_notes(raw: &str) -> String {
    let text: String = strip_html_comments(raw)
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !(c.is_control() || is_direction_control(*c)))
        .collect();
    // Trailing spaces go, a run of blank lines becomes one, and the text starts and ends on a line with something on it.
    let mut cleaned = String::with_capacity(text.len());
    let mut blank_lines = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank_lines += 1;
            if cleaned.is_empty() || blank_lines > 1 {
                continue;
            }
        } else {
            blank_lines = 0;
        }
        if !cleaned.is_empty() {
            cleaned.push('\n');
        }
        cleaned.push_str(line);
    }
    let cleaned = cleaned.trim();
    if cleaned.chars().count() <= NOTES_MAX_CHARS {
        return cleaned.to_string();
    }
    let mut cut: String = cleaned.chars().take(NOTES_MAX_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// `text` without its `<!-- … -->` comments. A comment that never ends takes the rest of the text with it.
fn strip_html_comments(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        kept.push_str(&rest[..start]);
        // From the second character: HTML ends `<!-->` and `<!--->` right where they begin.
        match rest[start + 2..].find("-->") {
            Some(end) => rest = &rest[start + 2 + end + 3..],
            None => return kept,
        }
    }
    kept.push_str(rest);
    kept
}

/// The characters that turn the text around or hide where it ends (`U+202A..=U+202E`, `U+2066..=U+2069`).
fn is_direction_control(c: char) -> bool {
    matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// The three numbers of a plain `X.Y.Z` version. Nothing else counts: no `v` in front, no pre-release or build part, no
/// leading zeros, and nothing that isn't a digit, so the result can go into a URL.
pub fn plain_version(version: &str) -> Option<[u32; 3]> {
    let mut numbers = [0u32; 3];
    let mut parts = version.split('.');
    for number in &mut numbers {
        let part = parts.next()?;
        // Up to nine digits keeps it in a `u32`; no leading zero keeps one spelling per version.
        let plain = !part.is_empty()
            && part.len() <= 9
            && part.bytes().all(|b| b.is_ascii_digit())
            && (part == "0" || !part.starts_with('0'));
        if !plain {
            return None;
        }
        *number = part.parse().ok()?;
    }
    parts.next().is_none().then_some(numbers)
}

/// The release page of `version` on this project's GitHub, or `None` when `version` isn't a plain `X.Y.Z`. The link
/// is built from a fixed base and the checked version, never taken from the release information.
pub fn release_url(version: &str) -> Option<String> {
    plain_version(version).map(|_| format!("{LINKS}releases/tag/v{version}"))
}

/// Whether `candidate` (a plain `X.Y.Z`) is newer than the running `current` version. A pre-release or build part of
/// `current` (`0.2.0-dev`) is ignored. When `current` can't be read at all the updater's own comparison decides, so
/// this says yes.
pub fn is_newer(current: &str, candidate: [u32; 3]) -> bool {
    let release = current.split(['-', '+']).next().unwrap_or_default();
    plain_version(release).is_none_or(|current| candidate > current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_comments_are_removed() {
        assert_eq!(clean_notes("Fixes a bug.<!-- macos-untested -->"), "Fixes a bug.");
        assert_eq!(clean_notes("<!-- a -->one<!-- b\nc -->two"), "onetwo");
    }

    #[test]
    fn a_comment_that_never_ends_takes_the_rest_with_it() {
        assert_eq!(clean_notes("Before <!-- never closed\nAfter"), "Before");
    }

    #[test]
    fn a_comment_that_closes_at_once_is_a_comment() {
        // HTML ends `<!-->` and `<!--->` right there.
        assert_eq!(clean_notes("a<!-->b"), "ab");
        assert_eq!(clean_notes("a<!--->b"), "ab");
    }

    #[test]
    fn control_characters_are_removed_and_line_breaks_stay() {
        assert_eq!(clean_notes("a\u{0}b\u{7}c\u{1b}[31md"), "abc[31md");
        assert_eq!(clean_notes("one\r\ntwo\nthree\tfour"), "one\ntwo\nthree\tfour");
        assert_eq!(clean_notes("a\u{85}b"), "ab");
    }

    #[test]
    fn text_direction_overrides_are_removed() {
        assert_eq!(clean_notes("safe\u{202e}txet"), "safetxet");
        assert_eq!(clean_notes("a\u{2066}b\u{2069}c"), "abc");
    }

    #[test]
    fn blank_runs_collapse_and_the_ends_are_trimmed() {
        assert_eq!(clean_notes("\n\n  first  \n\n\n\nsecond\n\n"), "first\n\nsecond");
        assert_eq!(clean_notes("a<!-- x -->\n<!-- y -->\n\nb"), "a\n\nb");
    }

    #[test]
    fn nothing_is_left_of_nothing() {
        assert_eq!(clean_notes(""), "");
        assert_eq!(clean_notes("<!-- only a marker -->\n"), "");
    }

    #[test]
    fn notes_of_exactly_the_cap_are_kept() {
        let notes = "a".repeat(NOTES_MAX_CHARS);
        assert_eq!(clean_notes(&notes), notes);
    }

    #[test]
    fn longer_notes_are_cut_to_the_cap_with_an_ellipsis() {
        let cleaned = clean_notes(&"a".repeat(NOTES_MAX_CHARS + 1));
        assert_eq!(cleaned.chars().count(), NOTES_MAX_CHARS);
        assert!(cleaned.ends_with('…'));
        assert!(cleaned.starts_with("aaa"));
    }

    #[test]
    fn notes_the_manifest_script_already_cut_are_not_cut_again() {
        // The manifest holds at most 4000 bytes of notes and ends a cut one in "…" (three bytes).
        let cut = format!("{}…", "a".repeat(3997));
        assert_eq!(cut.len(), 4000);
        assert_eq!(clean_notes(&cut), cut);
        let cut = format!("{}…", "é".repeat(1998));
        assert_eq!(clean_notes(&cut), cut);
    }

    #[test]
    fn the_cap_counts_characters_not_bytes() {
        // Four-byte characters: a byte count would cut far too early, and slicing by bytes could panic.
        let notes = "😀".repeat(NOTES_MAX_CHARS);
        assert_eq!(clean_notes(&notes), notes);
        let cleaned = clean_notes(&"😀".repeat(NOTES_MAX_CHARS + 5));
        assert_eq!(cleaned.chars().count(), NOTES_MAX_CHARS);
    }

    #[test]
    fn html_in_the_notes_stays_text() {
        // The window shows it as text; nothing here pretends to sanitise markup.
        assert_eq!(clean_notes("<script>alert(1)</script>"), "<script>alert(1)</script>");
    }

    #[test]
    fn a_plain_version_is_three_numbers() {
        assert_eq!(plain_version("0.2.0"), Some([0, 2, 0]));
        assert_eq!(plain_version("10.20.30"), Some([10, 20, 30]));
        assert_eq!(plain_version("99.0.0"), Some([99, 0, 0]));
    }

    #[test]
    fn anything_else_is_not_a_plain_version() {
        for version in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "1.2.3-rc.1",
            "1.2.3+build",
            "v1.2.3",
            "../x",
            "1.2.x",
            "1..3",
            " 1.2.3",
            "1.2.3 ",
            "1.2.3\n",
            "-1.2.3",
            "01.2.3",
            "1.02.3",
            "1.2.03",
            "1.2.3/../x",
            "٣.٢.١",
            "99999999999.0.0",
        ] {
            assert_eq!(plain_version(version), None, "{version:?}");
        }
    }

    #[test]
    fn the_release_page_is_built_from_the_fixed_base() {
        assert_eq!(
            release_url("0.3.0").as_deref(),
            Some("https://github.com/Roboroads/repeat-that/releases/tag/v0.3.0")
        );
        for version in ["1.2", "1.2.3-rc.1", "../x", "", "1.2.3/../../other"] {
            assert_eq!(release_url(version), None, "{version:?}");
        }
    }

    #[test]
    fn the_release_page_is_a_page_the_window_may_open() {
        assert!(crate::commands::is_project_link(&release_url("12.34.56").unwrap()));
    }

    #[test]
    fn newer_means_a_greater_number_at_the_first_place_that_differs() {
        assert!(is_newer("0.1.0", [0, 1, 1]));
        assert!(is_newer("0.1.9", [0, 2, 0]));
        assert!(is_newer("0.9.9", [1, 0, 0]));
        assert!(is_newer("0.2.0", [0, 10, 0]), "numbers, not text");
    }

    #[test]
    fn the_same_or_an_older_version_is_not_newer() {
        assert!(!is_newer("0.2.0", [0, 2, 0]));
        assert!(!is_newer("0.2.0", [0, 1, 9]));
        assert!(!is_newer("1.0.0", [0, 99, 99]));
    }

    #[test]
    fn a_pre_release_of_the_running_version_is_read_as_that_version() {
        assert!(!is_newer("0.2.0-dev", [0, 2, 0]));
        assert!(is_newer("0.2.0-dev", [0, 2, 1]));
        assert!(!is_newer("0.2.0+local", [0, 2, 0]));
    }

    #[test]
    fn a_running_version_that_cannot_be_read_leaves_it_to_the_updater() {
        assert!(is_newer("whatever", [0, 0, 1]));
        assert!(is_newer("", [0, 0, 1]));
    }
}
