//! Release notes arrive as HTML (a `releaseNotesLink` page or the item's
//! `<description>`). There's no HTML view here, so reduce them to readable
//! text: blocks become lines, list items become bullets, the rest is dropped.

fn decode_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        "ndash" => Some('–'),
        "mdash" => Some('—'),
        "hellip" => Some('…'),
        "rsquo" => Some('’'),
        "lsquo" => Some('‘'),
        "rdquo" => Some('”'),
        "ldquo" => Some('“'),
        _ => {
            let num = entity.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

pub fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    // Inside <script>/<style>/<head>: drop everything up to the closing tag.
    let mut skip_until: Option<&str> = None;
    while let Some(c) = rest.chars().next() {
        if c == '<' {
            let Some(end) = rest.find('>') else { break };
            let tag = &rest[1..end];
            rest = &rest[end + 1..];
            if tag.starts_with("!--") && !tag.ends_with("--") {
                rest = rest.find("-->").map_or("", |i| &rest[i + 3..]);
                continue;
            }
            let closing = tag.starts_with('/');
            let name: String = tag
                .trim_start_matches('/')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase();
            if let Some(until) = skip_until {
                if closing && name == until {
                    skip_until = None;
                }
                continue;
            }
            match name.as_str() {
                "script" | "style" | "head" | "title" if !closing => {
                    skip_until = Some(match name.as_str() {
                        "script" => "script",
                        "style" => "style",
                        "head" => "head",
                        _ => "title",
                    })
                }
                "br" => out.push('\n'),
                "li" if !closing => out.push_str("\n• "),
                "p" | "div" | "ul" | "ol" | "li" | "tr" | "pre" | "blockquote" | "hr" => out.push('\n'),
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => out.push_str("\n\n"),
                _ => {}
            }
            continue;
        }
        if skip_until.is_some() {
            rest = &rest[c.len_utf8()..];
            continue;
        }
        if c == '&' {
            if let Some(semi) = rest[..rest.len().min(12)].find(';') {
                if let Some(ch) = decode_entity(&rest[1..semi]) {
                    out.push(ch);
                    rest = &rest[semi + 1..];
                    continue;
                }
            }
        }
        // HTML whitespace collapses to one space.
        if c.is_whitespace() {
            if !out.ends_with([' ', '\n']) && !out.is_empty() {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
        rest = &rest[c.len_utf8()..];
    }

    // Tidy: trim each line, at most one blank line in a row.
    let mut text = String::new();
    let mut blank = 0;
    for line in out.lines().map(str::trim) {
        if line.is_empty() {
            blank += 1;
            continue;
        }
        if !text.is_empty() {
            text.push_str(if blank > 0 && !line.starts_with('•') { "\n\n" } else { "\n" });
        }
        text.push_str(line);
        blank = 0;
    }
    text
}

#[cfg(test)]
mod tests {
    use super::html_to_text;

    #[test]
    fn reads_pandoc_notes() {
        let html = "<h3 id=\"fixes\">Fixes</h3>\n<ul>\n<li><strong>Chat</strong> now reaches every viewer</li>\n<li>LTC &amp; MIDI fixes</li>\n</ul>\n<h3>Improvements</h3>\n<p>Faster   startup.</p>";
        assert_eq!(
            html_to_text(html),
            "Fixes\n• Chat now reaches every viewer\n• LTC & MIDI fixes\n\nImprovements\n\nFaster startup."
        );
    }

    #[test]
    fn drops_head_and_scripts() {
        let html = "<html><head><title>x</title><style>p{}</style></head><body><!-- hi --><p>A&#39;s &#x263A;</p><script>alert(1)</script></body></html>";
        assert_eq!(html_to_text(html), "A's ☺");
    }
}
