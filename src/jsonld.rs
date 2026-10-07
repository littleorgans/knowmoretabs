//! A page's JSON-LD: the blocks parsed, and every object in them walked.
//!
//! slice: enrich, content
//! why: Several readers want something out of a page's JSON-LD: `enrich`
//!      its types, `content` whether the publisher says it is not free, and
//!      the image picker the page's own image. Pages wrap the JSON in
//!      comments or CDATA and nest what they describe in `@graph`s, arrays
//!      and other objects, so the parsing and the walk are kept once, and
//!      each reader only says what it looks for.

use serde_json::{Map, Value};

/// Each `<script type="application/ld+json">` text that parses as JSON,
/// with the HTML comment or CDATA some pages wrap it in taken off.
pub fn parse(blocks: &[String]) -> Vec<Value> {
    blocks
        .iter()
        .filter_map(|block| {
            let text = block
                .trim()
                .trim_start_matches("<!--")
                .trim_end_matches("-->")
                .trim_start_matches("//<![CDATA[")
                .trim_end_matches("//]]>");
            serde_json::from_str(text).ok()
        })
        .collect()
}

/// Calls `visit` on every object in `value`, outermost first. `top` says
/// the object is one the page itself describes: the block, an item of a
/// top level array, or an item of a top level `@graph`, as opposed to an
/// object nested in one, such as its author or publisher.
pub fn walk(value: &Value, visit: &mut impl FnMut(&Map<String, Value>, bool)) {
    fn go(value: &Value, top: bool, visit: &mut impl FnMut(&Map<String, Value>, bool)) {
        match value {
            Value::Object(map) => {
                visit(map, top);
                for (key, inner) in map {
                    go(inner, top && key == "@graph", visit);
                }
            }
            Value::Array(items) => items.iter().for_each(|item| go(item, top, visit)),
            _ => {}
        }
    }
    go(value, true, visit);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_blocks_parse_and_others_are_dropped() {
        let blocks = [
            "<!--{\"@type\":\"WebPage\"}-->".to_owned(),
            "//<![CDATA[\n{\"@type\":\"Article\"}\n//]]>".to_owned(),
            "not json".to_owned(),
        ];
        let parsed = parse(&blocks);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1]["@type"], "Article");
    }

    #[test]
    fn the_walk_marks_what_the_page_describes_as_top_level() {
        let value: Value = serde_json::from_str(
            r#"[{"@type":"WebSite"},{"@graph":[{"@type":"Article","author":{"@type":"Person"}}]}]"#,
        )
        .unwrap();
        let mut seen = Vec::new();
        walk(&value, &mut |map, top| {
            let kind = map.get("@type").and_then(Value::as_str).unwrap_or("-");
            seen.push((kind.to_owned(), top));
        });
        assert_eq!(
            seen,
            [
                ("WebSite".to_owned(), true),
                ("-".to_owned(), true),
                ("Article".to_owned(), true),
                ("Person".to_owned(), false),
            ]
        );
    }
}
