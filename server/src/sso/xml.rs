//! What SAML needs from XML: exclusive canonicalization (the form signatures
//! are made over, https://www.w3.org/TR/xml-exc-c14n/), finding elements, and
//! the dates SAML writes.

use std::collections::BTreeMap;

use roxmltree::{Node, NodeId};

use crate::error::{Error, Result};

pub const XMLNS_XML: &str = "http://www.w3.org/XML/1998/namespace";

/// Parses a document the way every SAML message is read: no DTD (so no
/// entities), and a cap on its size in nodes.
pub fn parse(text: &str) -> Result<roxmltree::Document<'_>> {
    let options = roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: 200_000, ..Default::default() };
    roxmltree::Document::parse_with_options(text, options)
        .map_err(|err| Error::invalid(format!("the identity provider's answer isn't readable XML: {err}")))
}

/// The element children of `node` named `name` in namespace `ns`.
pub fn children<'a, 'input: 'a>(
    node: Node<'a, 'input>,
    ns: &'a str,
    name: &'a str,
) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == name && c.tag_name().namespace() == Some(ns))
}

/// The one element child of `node` named `name` in `ns`, if there's exactly one.
pub fn only_child<'a, 'input>(node: Node<'a, 'input>, ns: &'a str, name: &'a str) -> Option<Node<'a, 'input>> {
    let mut found = children(node, ns, name);
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

/// All the text inside an element, trimmed.
pub fn text_of(node: Node) -> String {
    node.descendants().filter(|n| n.is_text()).filter_map(|n| n.text()).collect::<String>().trim().to_string()
}

/// The prefix an element or attribute was written with, from its source.
fn prefix_of(qname: &str) -> &str {
    qname.split_once(':').map_or("", |(prefix, _)| prefix)
}

fn element_qname<'a>(source: &'a str, node: Node) -> &'a str {
    let start = &source[node.range().start + 1..];
    let end = start.find(|c: char| c.is_whitespace() || c == '>' || c == '/').unwrap_or(start.len());
    &start[..end]
}

/// `node` and everything in it in exclusive canonical form, without
/// comments unless `with_comments`, leaving out `skip` (the signature an
/// enveloped-signature transform takes away). `inclusive` is the
/// InclusiveNamespaces PrefixList ("#default" for the default namespace).
pub fn exc_c14n(source: &str, node: Node, skip: Option<NodeId>, inclusive: &[String], with_comments: bool) -> String {
    let mut out = String::new();
    let mut rendered = BTreeMap::new();
    rendered.insert(String::new(), String::new());
    write_node(source, node, skip, inclusive, with_comments, &rendered, &mut out);
    out
}

fn write_node(
    source: &str,
    node: Node,
    skip: Option<NodeId>,
    inclusive: &[String],
    with_comments: bool,
    rendered: &BTreeMap<String, String>,
    out: &mut String,
) {
    if Some(node.id()) == skip {
        return;
    }
    if node.is_text() {
        escape_text(node.text().unwrap_or_default(), out);
        return;
    }
    if node.is_comment() {
        if with_comments {
            out.push_str("<!--");
            out.push_str(node.text().unwrap_or_default());
            out.push_str("-->");
        }
        return;
    }
    if node.is_pi() {
        if let Some(pi) = node.pi() {
            out.push_str("<?");
            out.push_str(pi.target);
            if let Some(value) = pi.value.filter(|v| !v.is_empty()) {
                out.push(' ');
                out.push_str(value);
            }
            out.push_str("?>");
        }
        return;
    }
    if !node.is_element() {
        return;
    }

    let qname = element_qname(source, node);
    let in_scope = |prefix: &str| -> String {
        if prefix.is_empty() && prefix_of(qname).is_empty() {
            return node.tag_name().namespace().unwrap_or_default().to_string();
        }
        let name = (!prefix.is_empty()).then_some(prefix);
        node.namespaces().find(|ns| ns.name() == name).map(|ns| ns.uri().to_string()).unwrap_or_default()
    };

    // The namespaces this element uses visibly: its own, its attributes'
    // (never xml:), and those the PrefixList names that are in scope.
    let mut used: Vec<String> = vec![prefix_of(qname).to_string()];
    for attribute in node.attributes() {
        let prefix = prefix_of(&source[attribute.range_qname()]);
        if !prefix.is_empty() && prefix != "xml" {
            used.push(prefix.to_string());
        }
    }
    for prefix in inclusive {
        let prefix = if prefix == "#default" { String::new() } else { prefix.clone() };
        if prefix == "xml" {
            continue;
        }
        let declared = if prefix.is_empty() {
            node.namespaces().any(|ns| ns.name().is_none())
        } else {
            node.namespaces().any(|ns| ns.name() == Some(prefix.as_str()))
        };
        if declared {
            used.push(prefix);
        }
    }
    used.sort();
    used.dedup();

    let mut context = rendered.clone();
    let mut declarations = Vec::new();
    for prefix in used {
        let uri = in_scope(&prefix);
        if context.get(&prefix).map(String::as_str) == Some(uri.as_str()) {
            continue;
        }
        context.insert(prefix.clone(), uri.clone());
        declarations.push((prefix, uri));
    }

    out.push('<');
    out.push_str(qname);
    for (prefix, uri) in &declarations {
        out.push_str(if prefix.is_empty() { " xmlns" } else { " xmlns:" });
        out.push_str(prefix);
        out.push_str("=\"");
        escape_attribute(uri, out);
        out.push('"');
    }
    let mut attributes: Vec<_> = node
        .attributes()
        .map(|a| (a.namespace().unwrap_or_default(), a.name(), &source[a.range_qname()], a.value()))
        .collect();
    attributes.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    for (_, _, qname, value) in attributes {
        out.push(' ');
        out.push_str(qname);
        out.push_str("=\"");
        escape_attribute(value, out);
        out.push('"');
    }
    out.push('>');
    for child in node.children() {
        write_node(source, child, skip, inclusive, with_comments, &context, out);
    }
    out.push_str("</");
    out.push_str(qname);
    out.push('>');
}

fn escape_text(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#xD;"),
            c => out.push(c),
        }
    }
}

fn escape_attribute(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            '\t' => out.push_str("&#x9;"),
            '\n' => out.push_str("&#xA;"),
            '\r' => out.push_str("&#xD;"),
            c => out.push(c),
        }
    }
}

/// Escapes text for an XML document this side writes.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    escape_attribute(text, &mut out);
    out.replace('>', "&gt;")
}

/// A time as SAML writes it (xs:dateTime in UTC): 2026-10-03T01:20:00Z.
pub fn format_time(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

/// Reads an xs:dateTime as unix milliseconds: fractions of a second and a
/// zone offset are allowed; no zone reads as UTC.
pub fn parse_time(text: &str) -> Option<i64> {
    let text = text.trim();
    let (date, time) = text.split_once('T')?;
    let mut date_parts = date.splitn(3, '-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    let (clock, offset_secs) = if let Some(clock) = time.strip_suffix('Z') {
        (clock, 0)
    } else if let Some(at) = time.rfind(['+', '-']).filter(|&at| at >= 8) {
        let (clock, zone) = time.split_at(at);
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        let (h, m) = zone[1..].split_once(':')?;
        (clock, sign * (h.parse::<i64>().ok()? * 3600 + m.parse::<i64>().ok()? * 60))
    } else {
        (time, 0)
    };
    let mut clock_parts = clock.splitn(3, ':');
    let hour: i64 = clock_parts.next()?.parse().ok()?;
    let minute: i64 = clock_parts.next()?.parse().ok()?;
    let seconds = clock_parts.next()?;
    let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let second: i64 = whole.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let millis: i64 = format!("{:0<3}", &fraction[..fraction.len().min(3)]).parse().ok()?;
    let days = days_from_civil(year, month, day);
    Some(((days * 86_400 + hour * 3600 + minute * 60 + second) - offset_secs) * 1000 + millis)
}

// Howard Hinnant's civil calendar algorithms.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let month = i64::from(month);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c14n(text: &str, path: &[&str]) -> String {
        let doc = parse(text).unwrap();
        let mut node = doc.root_element();
        for name in path {
            node = node.children().find(|c| c.is_element() && c.tag_name().name() == *name).unwrap();
        }
        exc_c14n(text, node, None, &[], false)
    }

    #[test]
    fn canonical_form_follows_the_spec() {
        // Namespaces only where they're used, attributes sorted by namespace
        // then name, empty elements written out, comments dropped, escapes.
        let text = r#"<a:Root xmlns:a="urn:a" xmlns:b="urn:b" xmlns:unused="urn:u"><a:Child z="1" b:y="2" a="&amp;&#10;" ><!-- hi --><b:Leaf/>x &lt; y &gt; z</a:Child></a:Root>"#;
        assert_eq!(
            c14n(text, &["Child"]),
            r#"<a:Child xmlns:a="urn:a" xmlns:b="urn:b" a="&amp;&#xA;" z="1" b:y="2"><b:Leaf></b:Leaf>x &lt; y &gt; z</a:Child>"#
        );
    }

    #[test]
    fn default_namespaces_are_declared_once() {
        let text = r#"<Root xmlns="urn:d"><Child><Inner xmlns="">t</Inner></Child></Root>"#;
        assert_eq!(c14n(text, &["Child"]), r#"<Child xmlns="urn:d"><Inner xmlns="">t</Inner></Child>"#);
        let text = r#"<Root><Child>t</Child></Root>"#;
        assert_eq!(c14n(text, &["Child"]), "<Child>t</Child>");
    }

    #[test]
    fn inclusive_prefixes_are_kept() {
        let doc_text = r#"<r xmlns:x="urn:x" xmlns:y="urn:y"><c/></r>"#;
        let doc = parse(doc_text).unwrap();
        let c = doc.root_element().first_element_child().unwrap();
        assert_eq!(exc_c14n(doc_text, c, None, &["x".into(), "z".into()], false), r#"<c xmlns:x="urn:x"></c>"#);
    }

    #[test]
    fn dtds_are_refused() {
        assert!(parse(r#"<!DOCTYPE r [<!ENTITY e "boom">]><r>&e;</r>"#).is_err());
    }

    #[test]
    fn times_round_trip() {
        assert_eq!(format_time(0), "1970-01-01T00:00:00Z");
        let t = parse_time("2026-10-03T01:20:00Z").unwrap();
        assert_eq!(format_time(t), "2026-10-03T01:20:00Z");
        assert_eq!(parse_time("2026-10-03T01:20:00.5Z"), Some(t + 500));
        assert_eq!(parse_time("2026-10-03T03:20:00+02:00"), Some(t));
        assert_eq!(parse_time("2024-02-29T00:00:00Z").map(format_time).as_deref(), Some("2024-02-29T00:00:00Z"));
        assert_eq!(parse_time("nope"), None);
    }
}
