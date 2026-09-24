//! Builds a synthetic Chrome `History` database for tests, from the schema
//! text committed in `tests/fixtures/history-v70.sql`. Every URL and search
//! term is invented; no row here ever comes from a real profile, and no
//! database file is committed.

#![allow(dead_code)]

use std::path::Path;

use rusqlite::{Connection, params};

/// Chrome 153's `CREATE` statements for the tables `save` reads.
pub const SCHEMA_V70: &str = include_str!("../fixtures/history-v70.sql");

/// Microseconds from 1601-01-01, Chrome's epoch, to 1970-01-01.
const WINDOWS_EPOCH_OFFSET_US: i64 = 11_644_473_600 * 1_000_000;

/// Chrome's `base::Time` for an RFC 3339 instant.
pub fn chrome_time(at: &str) -> i64 {
    let at: jiff::Timestamp = at.parse().expect("RFC 3339 time");
    at.as_microsecond() + WINDOWS_EPOCH_OFFSET_US
}

/// Appends rows the way Chrome's history backend would. Ids are SQLite's.
pub struct HistoryBuilder {
    pub conn: Connection,
}

impl HistoryBuilder {
    /// A version-70 History at `path`, with no rows but `meta`.
    pub fn create(path: &Path) -> Self {
        let conn = Connection::open(path).expect("create History");
        conn.execute_batch(SCHEMA_V70).expect("v70 schema");
        conn.execute(
            "insert into meta (key, value) values ('version', '70'), ('last_compatible_version', '16')",
            [],
        )
        .expect("meta");
        Self { conn }
    }

    /// The shape of a History from before schema 49: no `opener_visit`, no
    /// `external_referrer_url`, no `context_annotations`.
    pub fn without_newer_columns(self) -> Self {
        self.conn
            .execute_batch(
                "alter table visits drop column opener_visit;
                 alter table visits drop column external_referrer_url;
                 drop table context_annotations;
                 update meta set value = '48' where key = 'version';",
            )
            .expect("older shape");
        self
    }

    /// A `urls` row; returns its id.
    pub fn url(&self, url: &str, visits: i64, typed: i64, last_visit: &str) -> i64 {
        self.conn
            .execute(
                "insert into urls (url, title, visit_count, typed_count, last_visit_time)
                 values (?1, '', ?2, ?3, ?4)",
                params![url, visits, typed, chrome_time(last_visit)],
            )
            .expect("url");
        self.conn.last_insert_rowid()
    }

    /// A `visits` row. `from` and `opener` are visit ids, 0 for none, as
    /// Chrome writes them.
    pub fn visit(&self, url_id: i64, at: &str, from: i64, opener: i64) -> i64 {
        self.conn
            .execute(
                "insert into visits (url, visit_time, from_visit, opener_visit)
                 values (?1, ?2, ?3, ?4)",
                params![url_id, chrome_time(at), from, opener],
            )
            .expect("visit");
        self.conn.last_insert_rowid()
    }

    /// A visit on a History without `opener_visit`.
    pub fn old_visit(&self, url_id: i64, at: &str, from: i64) -> i64 {
        self.conn
            .execute(
                "insert into visits (url, visit_time, from_visit) values (?1, ?2, ?3)",
                params![url_id, chrome_time(at), from],
            )
            .expect("visit");
        self.conn.last_insert_rowid()
    }

    pub fn external_referrer(&self, visit_id: i64, url: &str) {
        self.conn
            .execute(
                "update visits set external_referrer_url = ?2 where id = ?1",
                params![visit_id, url],
            )
            .expect("external referrer");
    }

    /// Marks `url_id` as a search results page for `term`.
    pub fn search(&self, url_id: i64, term: &str) {
        self.conn
            .execute(
                "insert into keyword_search_terms (keyword_id, url_id, term, normalized_term)
                 values (2, ?1, ?2, lower(?2))",
                params![url_id, term],
            )
            .expect("search term");
    }

    /// The visit's foreground time in microseconds; Chrome writes
    /// -1,000,000 for "unknown".
    pub fn foreground(&self, visit_id: i64, micros: i64) {
        self.conn
            .execute(
                "insert into context_annotations (visit_id, context_annotation_flags,
                     total_foreground_duration) values (?1, 0, ?2)",
                params![visit_id, micros],
            )
            .expect("context annotation");
    }
}
