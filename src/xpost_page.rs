//! An X post, or an author's thread of them, as the markdown a page keeps:
//! one renderer for every reader of X.
//!
//! slice: content, x-bookmarks
//! why: The same post reaches the library from the public post API and
//!      from the page the owner was reading on X, and a reader of the
//!      library should not see which: the same byline, the same text, the
//!      same quote and image descriptions, in the same shape. So each
//!      reader maps what it read to one neutral post, and this file alone
//!      draws it. One post renders as the X route always has; a thread is
//!      one titled page, each post its own byline and body, in the order
//!      it was read, divided by a rule.

use jiff::Timestamp;

/// A post, as any reader of X saw it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Post {
    /// Who wrote it; none when the reader could not say.
    pub author: Option<Author>,
    pub at: Option<Timestamp>,
    pub text: String,
    pub article: Option<Article>,
    /// Each image description, with what it describes (`Image`, `GIF`).
    pub alts: Vec<(&'static str, String)>,
    pub quote: Option<Box<Post>>,
    /// Why the post cannot be read (`post deleted`, `protected account`);
    /// such a post keeps no text.
    pub unavailable: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Author {
    pub name: String,
    pub handle: String,
}

/// An X article: its title and its paragraphs, already markdown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Article {
    pub title: String,
    pub parts: Vec<String>,
}

/// The page's title: the first post's article title, or who wrote it.
pub fn title(posts: &[Post]) -> String {
    let Some(post) = posts.first() else {
        return "Someone on X".to_owned();
    };
    match &post.article {
        Some(article) if !article.title.trim().is_empty() => article.title.trim().to_owned(),
        _ => format!("{} on X", byline_name(post)),
    }
}

/// Every piece of text a post holds, quoted post included, joined; empty
/// when it holds none.
pub fn text_of(post: &Post) -> String {
    if post.unavailable.is_some() {
        return String::new();
    }
    let mut parts = vec![post.text.trim().to_owned()];
    if let Some(article) = &post.article {
        parts.push(article.title.trim().to_owned());
        parts.extend(article.parts.iter().cloned());
    }
    parts.extend(post.alts.iter().map(|(_, alt)| alt.clone()));
    if let Some(quote) = &post.quote {
        parts.push(text_of(quote));
    }
    parts.retain(|part| !part.is_empty());
    parts.join("\n")
}

/// `# Title`, then each post: its byline, text, article and image
/// descriptions, and the post it quotes as a block quote; posts divided by
/// a rule. Ends in one newline.
pub fn markdown(posts: &[Post]) -> String {
    let sections: Vec<String> = posts.iter().map(section).collect();
    let text = format!("# {}\n\n{}", title(posts), sections.join("\n\n---\n\n"));
    format!("{}\n", text.trim_end())
}

/// One post and its quote; a post that cannot be read is one line saying
/// why.
fn section(post: &Post) -> String {
    if let Some(reason) = &post.unavailable {
        return format!("{}.", capitalized(reason));
    }
    let mut text = body(post, false);
    if let Some(quote) = &post.quote {
        let quoted = match &quote.unavailable {
            Some(reason) => format!("Quoted {reason}."),
            None => format!("Quoting {}", body(quote, true)),
        };
        text.push_str("\n\n");
        for line in quoted.trim_end().lines() {
            text.push_str(if line.is_empty() { ">" } else { "> " });
            text.push_str(line);
            text.push('\n');
        }
    }
    text.trim_end().to_owned()
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn byline_name(post: &Post) -> String {
    match &post.author {
        Some(author) if !author.name.is_empty() => format!("{} (@{})", author.name, author.handle),
        Some(author) => format!("@{}", author.handle),
        None => "Someone".to_owned(),
    }
}

/// `Name (@handle), 2026-10-07T09:00:00Z`.
fn byline(post: &Post) -> String {
    match post.at {
        Some(when) => format!("{}, {when}", byline_name(post)),
        None => byline_name(post),
    }
}

/// A post without its quote: the byline, then each part a paragraph. A
/// quoted article's title is a heading; the page's own is its title. The
/// text is trimmed at its edges only: after a blank line, a leading indent
/// of four spaces would make it a code block.
fn body(post: &Post, quoted: bool) -> String {
    let mut parts = vec![byline(post)];
    if !post.text.trim().is_empty() {
        parts.push(post.text.trim().to_owned());
    }
    if let Some(article) = &post.article {
        if quoted && !article.title.trim().is_empty() {
            parts.push(format!("## {}", article.title.trim()));
        }
        parts.extend(article.parts.iter().cloned());
    }
    parts.extend(post.alts.iter().map(|(kind, alt)| format!("{kind}: {alt}")));
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(handle: &str, text: &str) -> Post {
        Post {
            author: Some(Author {
                name: "Example Author".to_owned(),
                handle: handle.to_owned(),
            }),
            at: Some("2026-01-01T10:00:00Z".parse().unwrap()),
            text: text.to_owned(),
            ..Post::default()
        }
    }

    #[test]
    fn a_thread_is_one_page_each_post_its_own_byline_divided_by_a_rule() {
        let mut second = post("example_author", "Second.");
        second.quote = Some(Box::new(Post {
            author: Some(Author {
                name: String::new(),
                handle: "example_quote".to_owned(),
            }),
            text: "Quoted.".to_owned(),
            ..Post::default()
        }));
        let gone = Post {
            unavailable: Some("post deleted".to_owned()),
            ..Post::default()
        };
        assert_eq!(
            markdown(&[post("example_author", "First."), second, gone]),
            "# Example Author (@example_author) on X\n\n\
             Example Author (@example_author), 2026-01-01T10:00:00Z\n\nFirst.\n\n---\n\n\
             Example Author (@example_author), 2026-01-01T10:00:00Z\n\nSecond.\n\n\
             > Quoting @example_quote\n>\n> Quoted.\n\n---\n\n\
             Post deleted.\n"
        );
    }

    #[test]
    fn a_post_without_an_author_is_someones_and_text_skips_what_cannot_be_read() {
        let mut nobody = post("x", "Words.");
        nobody.author = None;
        assert_eq!(title(&[nobody]), "Someone on X");
        let mut quoting = post("example_author", "Mine.");
        quoting.quote = Some(Box::new(Post {
            text: "Hidden.".to_owned(),
            unavailable: Some("protected account".to_owned()),
            ..Post::default()
        }));
        assert_eq!(text_of(&quoting), "Mine.");
    }
}
