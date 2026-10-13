//! Preview images through `serve`: the library names only the kept images
//! the image route can serve, and the route serves those names and nothing
//! else, not through a symlink or a directory.

mod common;

use std::fs;

use common::Fixture;
use common::serve::{A, B, HIDDEN, Server, archive, kept_image, kept_name, page};

#[test]
fn the_library_names_kept_images_and_the_image_route_serves_only_those_names() {
    let fx = Fixture::new();
    archive(&fx);
    kept_image(&fx, A, "ok", Some(b"jpeg a"));
    kept_image(&fx, B, "ok", None);
    kept_image(&fx, HIDDEN, "ok", Some(b"jpeg hidden"));
    kept_image(&fx, HIDDEN, "none", None);
    let server = Server::start(&fx);
    let library = server.library();
    let a = kept_name(A);
    assert_eq!(page(&library, A)["image"], a.as_str());
    assert!(page(&library, B).get("image").is_none(), "ok, but no file");
    assert!(
        page(&library, HIDDEN).get("image").is_none(),
        "the latest line is not ok"
    );

    for path in [format!("/api/image/{a}"), format!("/api/image/{a}?v=1")] {
        let reply = server.get(&path);
        assert_eq!(reply.status, 200, "{path}");
        assert_eq!(reply.header("content-type"), Some("image/jpeg"));
        assert_eq!(reply.header("content-length"), Some("6"));
        assert_eq!(reply.header("cache-control"), Some("no-store"));
        assert_eq!(reply.header("x-content-type-options"), Some("nosniff"));
        assert!(reply.header("access-control-allow-origin").is_none());
        assert_eq!(reply.body, b"jpeg a");
    }
    let encoded = format!("{}%2F{}", &a[..30], &a[33..]);
    for name in [
        a.to_uppercase(),
        a[..63].to_owned(),
        format!("{a}0"),
        format!("{a}.jpg"),
        format!("{a}/"),
        format!("../images/{a}"),
        "..%2F..%2Flibrary.json".to_owned(),
        format!("%2e%2e%2f{}", &a[9..]),
        encoded,
        format!("{}%5C{}", &a[..30], &a[33..]),
        kept_name(B),
        String::new(),
    ] {
        let reply = server.get(&format!("/api/image/{name}"));
        assert_eq!(reply.status, 404, "{name}");
        assert_eq!(reply.header("content-type"), Some("application/json"));
    }
}

#[cfg(unix)]
#[test]
fn the_image_route_does_not_follow_a_symlink_out_of_the_store() {
    let fx = Fixture::new();
    archive(&fx);
    kept_image(&fx, A, "ok", None);
    let outside = fx.root.join("private.txt");
    fs::write(&outside, b"private bytes").unwrap();
    std::os::unix::fs::symlink(
        outside,
        fx.root
            .join("pages/images")
            .join(format!("{}.jpg", kept_name(A))),
    )
    .unwrap();
    let server = Server::start(&fx);
    let reply = server.get(&format!("/api/image/{}", kept_name(A)));
    assert_eq!(
        (
            reply.status,
            page(&server.library(), A).get("image").is_none()
        ),
        (404, true),
        "symlinks are neither served nor advertised"
    );
    assert_eq!(reply.header("cache-control"), Some("no-store"));
    assert_eq!(reply.header("x-content-type-options"), Some("nosniff"));
    assert!(
        !reply
            .body
            .windows(13)
            .any(|bytes| bytes == b"private bytes")
    );
}

#[test]
fn the_library_and_image_route_ignore_directories_named_as_images() {
    let fx = Fixture::new();
    archive(&fx);
    kept_image(&fx, A, "ok", None);
    fs::create_dir(
        fx.root
            .join("pages/images")
            .join(format!("{}.jpg", kept_name(A))),
    )
    .unwrap();
    let server = Server::start(&fx);
    assert!(page(&server.library(), A).get("image").is_none());
    assert_eq!(
        server.get(&format!("/api/image/{}", kept_name(A))).status,
        404
    );
}
