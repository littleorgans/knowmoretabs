-- Chrome's History schema, version 70: the CREATE statements for the five
-- tables `save` reads or probes, copied verbatim from `sqlite_schema` of a
-- copy of Chrome 153's History (Default profile, 2026-09-24). Schema text
-- only, no rows. Tests build a History from this and fill it with synthetic
-- `example.test` rows; see tests/common/history_builder.rs.
CREATE TABLE context_annotations(visit_id INTEGER PRIMARY KEY,context_annotation_flags INTEGER NOT NULL,duration_since_last_visit INTEGER,page_end_reason INTEGER,total_foreground_duration INTEGER,browser_type INTEGER DEFAULT 0 NOT NULL,window_id INTEGER DEFAULT -1 NOT NULL,tab_id INTEGER DEFAULT -1 NOT NULL,task_id INTEGER DEFAULT -1 NOT NULL,root_task_id INTEGER DEFAULT -1 NOT NULL,parent_task_id INTEGER DEFAULT -1 NOT NULL,response_code INTEGER DEFAULT 0 NOT NULL);
CREATE TABLE keyword_search_terms (keyword_id INTEGER NOT NULL,url_id INTEGER NOT NULL,term LONGVARCHAR NOT NULL,normalized_term LONGVARCHAR NOT NULL);
CREATE INDEX keyword_search_terms_index1 ON keyword_search_terms (keyword_id, normalized_term);
CREATE INDEX keyword_search_terms_index2 ON keyword_search_terms (url_id);
CREATE INDEX keyword_search_terms_index3 ON keyword_search_terms (term);
CREATE TABLE meta(key LONGVARCHAR NOT NULL UNIQUE PRIMARY KEY, value LONGVARCHAR);
CREATE TABLE urls(id INTEGER PRIMARY KEY AUTOINCREMENT,url LONGVARCHAR,title LONGVARCHAR,visit_count INTEGER DEFAULT 0 NOT NULL,typed_count INTEGER DEFAULT 0 NOT NULL,last_visit_time INTEGER NOT NULL,hidden INTEGER DEFAULT 0 NOT NULL);
CREATE INDEX urls_url_index ON urls (url);
CREATE TABLE visits(id INTEGER PRIMARY KEY AUTOINCREMENT,url INTEGER NOT NULL,visit_time INTEGER NOT NULL,from_visit INTEGER,transition INTEGER DEFAULT 0 NOT NULL,segment_id INTEGER,visit_duration INTEGER DEFAULT 0 NOT NULL,incremented_omnibox_typed_score BOOLEAN DEFAULT FALSE NOT NULL,opener_visit INTEGER,originator_cache_guid TEXT,originator_visit_id INTEGER,originator_from_visit INTEGER,originator_opener_visit INTEGER,is_known_to_sync BOOLEAN DEFAULT FALSE NOT NULL,consider_for_ntp_most_visited BOOLEAN DEFAULT FALSE NOT NULL, external_referrer_url TEXT, visited_link_id INTEGER, app_id TEXT);
CREATE INDEX visits_from_index ON visits (from_visit);
CREATE INDEX visits_originator_id_index ON visits (originator_visit_id);
CREATE INDEX visits_time_index ON visits (visit_time);
CREATE INDEX visits_url_index ON visits (url);
