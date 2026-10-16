//! Sparkle appcast parsing: an RSS feed whose items carry `sparkle:`
//! attributes and elements. Only what an AppImage update needs is read.

use crate::version;
use std::cmp::Ordering;

const SPARKLE_NS: &str = "http://www.andymatuschak.org/xml-namespaces/sparkle";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    pub title: String,
    /// Machine version, compared against the running one.
    pub version: String,
    /// What to show people (`sparkle:shortVersionString`), else `version`.
    pub display_version: String,
    pub url: String,
    /// Declared size in bytes; 0 when the feed doesn't say.
    pub length: u64,
    /// Base64 Ed25519 signature of the file at `url`.
    pub ed_signature: Option<String>,
    pub release_notes_link: Option<String>,
    /// Inline notes (`<description>`), usually HTML.
    pub description: Option<String>,
    /// A page to send people to when the update can't be installed in place.
    pub link: Option<String>,
}

fn sparkle<'a>(node: roxmltree::Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.attribute((SPARKLE_NS, name))
}

fn child<'a>(node: roxmltree::Node<'a, 'a>, ns: Option<&str>, name: &str) -> Option<roxmltree::Node<'a, 'a>> {
    node.children()
        .find(|c| c.is_element() && c.tag_name().name() == name && c.tag_name().namespace() == ns)
}

fn child_text(node: roxmltree::Node, ns: Option<&str>, name: &str) -> Option<String> {
    let text: String = child(node, ns, name)?
        .descendants()
        .filter(|d| d.is_text())
        .filter_map(|d| d.text())
        .collect();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Every item in the feed with an installable enclosure for Linux: no
/// `sparkle:os`, or `sparkle:os="linux"`.
pub fn parse(xml: &str) -> Result<Vec<Item>, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("the update feed isn't valid XML: {e}"))?;
    let channel = doc
        .descendants()
        .find(|n| n.has_tag_name("channel"))
        .ok_or("the update feed has no <channel>")?;
    let channel_link = child_text(channel, None, "link");

    let mut items = Vec::new();
    for node in channel.children().filter(|n| n.has_tag_name("item")) {
        let Some(enclosure) = node
            .children()
            .filter(|c| c.has_tag_name("enclosure"))
            .find(|e| sparkle(*e, "os").is_none_or(|os| os.eq_ignore_ascii_case("linux")))
        else {
            continue;
        };
        let Some(url) = enclosure.attribute("url") else {
            continue;
        };
        let Some(version) = sparkle(enclosure, "version")
            .map(str::to_string)
            .or_else(|| child_text(node, Some(SPARKLE_NS), "version"))
        else {
            continue;
        };
        let display_version = sparkle(enclosure, "shortVersionString")
            .map(str::to_string)
            .or_else(|| child_text(node, Some(SPARKLE_NS), "shortVersionString"))
            .unwrap_or_else(|| version.clone());
        items.push(Item {
            title: child_text(node, None, "title").unwrap_or_default(),
            version,
            display_version,
            url: url.to_string(),
            length: enclosure.attribute("length").and_then(|l| l.parse().ok()).unwrap_or(0),
            ed_signature: sparkle(enclosure, "edSignature").map(str::to_string),
            release_notes_link: child_text(node, Some(SPARKLE_NS), "releaseNotesLink"),
            description: child_text(node, None, "description"),
            link: child_text(node, None, "link").or_else(|| channel_link.clone()),
        });
    }
    Ok(items)
}

/// The newest item that's newer than `current`, if any.
pub fn newest_update(items: Vec<Item>, current: &str) -> Option<Item> {
    items
        .into_iter()
        .filter(|i| version::compare(&i.version, current) == Ordering::Greater)
        .max_by(|a, b| version::compare(&a.version, &b.version))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>App</title>
    <link>https://example.com/download</link>
    <item>
      <title>Version 1.2.0</title>
      <sparkle:releaseNotesLink>https://example.com/notes/1.2.0.html</sparkle:releaseNotesLink>
      <enclosure sparkle:edSignature="c2ln" url="https://example.com/App-1.2.0.tar.gz" sparkle:version="1.2.0" length="0" type="application/gzip" />
    </item>
    <item>
      <title>Version 1.3.0 (Windows only)</title>
      <enclosure sparkle:os="windows" url="https://example.com/App-1.3.0.exe" sparkle:version="1.3.0" />
    </item>
    <item>
      <title>Version 1.1.0</title>
      <sparkle:version>1.1.0</sparkle:version>
      <sparkle:shortVersionString>1.1</sparkle:shortVersionString>
      <description><![CDATA[<p>Fixes</p>]]></description>
      <enclosure sparkle:os="linux" url="https://example.com/App-1.1.0.AppImage" length="42" />
    </item>
  </channel>
</rss>"#;

    #[test]
    fn reads_linux_items() {
        let items = parse(FEED).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].version, "1.2.0");
        assert_eq!(items[0].ed_signature.as_deref(), Some("c2ln"));
        assert_eq!(items[0].release_notes_link.as_deref(), Some("https://example.com/notes/1.2.0.html"));
        assert_eq!(items[0].link.as_deref(), Some("https://example.com/download"));
        assert_eq!(items[1].version, "1.1.0");
        assert_eq!(items[1].display_version, "1.1");
        assert_eq!(items[1].length, 42);
        assert_eq!(items[1].description.as_deref(), Some("<p>Fixes</p>"));
    }

    #[test]
    fn picks_the_newest_newer_item() {
        let items = parse(FEED).unwrap();
        assert_eq!(newest_update(items.clone(), "1.0").unwrap().version, "1.2.0");
        assert_eq!(newest_update(items.clone(), "1.1.5").unwrap().version, "1.2.0");
        assert!(newest_update(items, "1.2.0").is_none());
    }
}
