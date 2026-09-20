//! The `Session_*` command table: folds Chrome's `SessionService` log into
//! windows, tabs and groups the way Chromium's own restore does.
//!
//! slice: capture
//! why: A session file is a fold, not a tree: the last write for a key wins,
//!      closes erase, and a window is real only once it has been given a
//!      type. This module reproduces that fold so that what we snapshot is
//!      what Chrome would restore. It is deliberately tolerant where Chromium
//!      is not: an unknown command, a bad payload or a tab with no usable
//!      navigation costs us that one record and a counter, never the run.
//!      `Tabs_*` files use a different table with colliding ids; selecting
//!      the table by file name is an explicit decision here, so slice 4 can
//!      add the second table without touching this one.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use jiff::Timestamp;

use crate::model::{DroppedTabs, Group, Stats, Tab, Window};
use crate::snss::{self, HeaderError, Pickle, Token, i32_at, i64_at, u64_at};

/// Which command-ID table a file uses. Chosen from the file-name prefix and
/// never guessed from contents, because ids collide between the two tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandTable {
    /// `Session_*` and `Apps_*`: open windows and tabs. Handled here.
    Session,
    /// `Tabs_*`: the recently-closed list. Different ids; refused until slice 4.
    Tabs,
}

impl CommandTable {
    /// `None` for a name without a recognised prefix; callers decide what a
    /// hand-copied file without one most plausibly is.
    pub fn for_file_name(name: &str) -> Option<Self> {
        if name.starts_with("Session_") || name.starts_with("Apps_") {
            Some(Self::Session)
        } else if name.starts_with("Tabs_") {
            Some(Self::Tabs)
        } else {
            None
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Tabs => "tabs",
        }
    }
}

/// `components/sessions/core/session_service_commands.cc`.
mod id {
    pub const SET_TAB_WINDOW: u8 = 0;
    pub const SET_TAB_INDEX_IN_WINDOW: u8 = 2;
    pub const NAV_PRUNED_FROM_BACK: u8 = 5;
    pub const UPDATE_TAB_NAVIGATION: u8 = 6;
    pub const SET_SELECTED_NAVIGATION_INDEX: u8 = 7;
    pub const SET_SELECTED_TAB_IN_INDEX: u8 = 8;
    pub const SET_WINDOW_TYPE: u8 = 9;
    pub const NAV_PRUNED_FROM_FRONT: u8 = 11;
    pub const SET_PINNED_STATE: u8 = 12;
    pub const TAB_CLOSED: u8 = 16;
    pub const WINDOW_CLOSED: u8 = 17;
    pub const LAST_ACTIVE_TIME: u8 = 21;
    pub const NAV_PRUNED: u8 = 24;
    pub const SET_TAB_GROUP: u8 = 25;
    pub const SET_TAB_GROUP_METADATA2: u8 = 27;
    pub const INITIAL_STATE_MARKER: u8 = 255;
    /// Ids Chromium defines that carry nothing we keep. Counted, not skipped
    /// as unknown, so a genuinely new id stands out in `unknown_command_ids`.
    pub const KNOWN_IGNORED: [u8; 21] = [
        1, 10, 13, 14, 15, 18, 19, 20, 22, 23, 26, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37,
    ];
}

/// Microseconds between 1601-01-01 (Chrome's epoch) and 1970-01-01.
const WINDOWS_EPOCH_OFFSET_US: i64 = 11_644_473_600 * 1_000_000;

/// Converts a Chrome `base::Time` (µs since 1601) to a timestamp; zero and
/// anything unrepresentable become `None`.
pub fn chrome_time(micros: i64) -> Option<Timestamp> {
    if micros <= 0 {
        return None;
    }
    let unix = micros.checked_sub(WINDOWS_EPOCH_OFFSET_US)?;
    Timestamp::from_microsecond(unix).ok()
}

#[derive(Debug, Clone)]
struct Navigation {
    url: String,
    title: String,
}

#[derive(Debug, Default)]
struct TabState {
    window_id: Option<i32>,
    position: Option<i32>,
    /// Chromium's `current_navigation_index` starts at -1 and is a
    /// controller index, not a list position; see `resolve_navigation`.
    selected: Option<i32>,
    pinned: bool,
    navigations: BTreeMap<i32, Navigation>,
    group: Option<Token>,
    last_active: Option<i64>,
}

#[derive(Debug, Default)]
struct GroupMeta {
    title: String,
    colour: u32,
    collapsed: bool,
    saved_guid: Option<String>,
}

/// The result of a parse: what will become the snapshot body, plus counters.
#[derive(Debug)]
pub struct Parsed {
    pub stats: Stats,
    pub windows: Vec<Window>,
    pub groups: Vec<Group>,
    pub tabs: Vec<Tab>,
}

#[derive(Debug, Default)]
struct Fold {
    tabs: BTreeMap<i32, TabState>,
    /// Windows that received `SetWindowType` and were not closed since.
    windows: BTreeMap<i32, i32>,
    closed_windows: BTreeSet<i32>,
    selected_tab: HashMap<i32, i32>,
    group_meta: HashMap<Token, GroupMeta>,
    stats: Stats,
    unknown_ids: BTreeSet<u8>,
}

/// Parses a `Session_*` file. Fails only on the header; everything past it
/// degrades into counters.
pub fn parse(bytes: &[u8]) -> Result<Parsed, HeaderError> {
    let version = snss::read_header(bytes)?;
    let framing = snss::frame(bytes);
    let mut fold = Fold::default();
    fold.stats.file_version = version;
    CommandTable::Session
        .name()
        .clone_into(&mut fold.stats.command_table);
    fold.stats.truncated_bytes = framing.truncated_bytes as u64;
    for command in &framing.commands {
        fold.apply(command.id, command.contents);
    }
    Ok(fold.resolve())
}

impl Fold {
    fn tab(&mut self, tab_id: i32) -> &mut TabState {
        self.tabs.entry(tab_id).or_default()
    }

    fn malformed(&mut self) {
        self.stats.malformed_commands += 1;
    }

    fn apply(&mut self, id: u8, contents: &[u8]) {
        self.stats.commands += 1;
        *self.stats.commands_by_id.entry(id).or_insert(0) += 1;
        let ok = match id {
            id::SET_TAB_WINDOW => pair(contents).map(|(window, tab)| {
                self.tab(tab).window_id = Some(window);
            }),
            id::SET_TAB_INDEX_IN_WINDOW => pair(contents).map(|(tab, index)| {
                self.tab(tab).position = Some(index);
            }),
            id::NAV_PRUNED_FROM_BACK => pair(contents).map(|(tab, index)| {
                self.tab(tab).navigations.retain(|&i, _| i < index);
            }),
            id::UPDATE_TAB_NAVIGATION => self.navigation(contents),
            id::SET_SELECTED_NAVIGATION_INDEX => pair(contents).map(|(tab, index)| {
                self.tab(tab).selected = Some(index);
            }),
            id::SET_SELECTED_TAB_IN_INDEX => pair(contents).map(|(window, index)| {
                self.selected_tab.insert(window, index);
            }),
            id::SET_WINDOW_TYPE => pair(contents).map(|(window, kind)| {
                self.windows.insert(window, kind);
                self.closed_windows.remove(&window);
            }),
            // Chromium validates `count > 0` and skips otherwise; so do we.
            id::NAV_PRUNED_FROM_FRONT => pair(contents)
                .filter(|&(_, count)| count > 0)
                .map(|(tab, count)| self.tab(tab).prune(0, count)),
            id::SET_PINNED_STATE => i32_at(contents, 0)
                .zip(contents.get(4))
                .map(|(tab, &flag)| self.tab(tab).pinned = flag != 0),
            id::TAB_CLOSED => i32_at(contents, 0).map(|tab| {
                self.tabs.remove(&tab);
            }),
            id::WINDOW_CLOSED => i32_at(contents, 0).map(|window| {
                self.windows.remove(&window);
                self.closed_windows.insert(window);
            }),
            id::LAST_ACTIVE_TIME => i32_at(contents, 0)
                .zip(i64_at(contents, 8))
                .map(|(tab, micros)| self.tab(tab).last_active = Some(micros)),
            id::NAV_PRUNED => i32_at(contents, 0)
                .zip(i32_at(contents, 4))
                .zip(i32_at(contents, 8))
                .filter(|&((_, index), count)| index >= 0 && count > 0)
                .map(|((tab, index), count)| self.tab(tab).prune(index, count)),
            id::SET_TAB_GROUP => self.tab_group(contents),
            id::SET_TAB_GROUP_METADATA2 => self.group_metadata(contents),
            id::INITIAL_STATE_MARKER => {
                self.stats.marker_count += 1;
                Some(())
            }
            other if id::KNOWN_IGNORED.contains(&other) => Some(()),
            other => {
                self.stats.unknown_commands += 1;
                self.unknown_ids.insert(other);
                Some(())
            }
        };
        if ok.is_none() {
            self.malformed();
        }
    }

    /// Command 6. Only `tab_id`, `index`, URL and title are read; the page
    /// state and everything after it are skipped, which is also what makes a
    /// record Chrome truncated at 64 KiB still usable.
    fn navigation(&mut self, contents: &[u8]) -> Option<()> {
        let mut pickle = Pickle::from_contents(contents)?;
        let tab = pickle.read_i32()?;
        let index = pickle.read_i32()?;
        let url = pickle.read_string()?;
        let title = pickle.read_string16().unwrap_or_else(|| {
            self.malformed();
            String::new()
        });
        self.tab(tab)
            .navigations
            .insert(index, Navigation { url, title });
        Some(())
    }

    /// Command 25, a raw struct with C padding: `has_group == false` means
    /// the tab explicitly left its group, and the last write wins.
    fn tab_group(&mut self, contents: &[u8]) -> Option<()> {
        let tab = i32_at(contents, 0)?;
        let high = u64_at(contents, 8)?;
        let low = u64_at(contents, 16)?;
        let has_group = *contents.get(24)? != 0;
        self.tab(tab).group = has_group.then_some(Token { high, low });
        Some(())
    }

    /// Command 27. Fields after the colour were appended in later milestones
    /// and are optional exactly as Chromium treats them.
    fn group_metadata(&mut self, contents: &[u8]) -> Option<()> {
        let mut pickle = Pickle::from_contents(contents)?;
        let token = pickle.read_token()?;
        let title = pickle.read_string16()?;
        let colour = pickle.read_u32()?;
        let collapsed = pickle.read_bool().unwrap_or(false);
        let saved_guid = pickle
            .read_bool()
            .filter(|&saved| saved)
            .and_then(|_| pickle.read_string());
        self.group_meta.insert(
            token,
            GroupMeta {
                title,
                colour,
                collapsed,
                saved_guid,
            },
        );
        Some(())
    }

    fn resolve(mut self) -> Parsed {
        let window_numbers: HashMap<i32, u32> = self
            .windows
            .keys()
            .enumerate()
            .map(|(i, &id)| (id, u32::try_from(i + 1).unwrap_or(u32::MAX)))
            .collect();
        let (mut tabs, group_windows, dropped) = self.resolve_tabs(&window_numbers);
        tabs.sort_by_key(|t| (t.window, t.position, t.tab_id));
        let windows = self.resolve_windows(&window_numbers, &mut tabs);
        let groups = self.resolve_groups(group_windows);

        let mut stats = self.stats;
        stats.unknown_command_ids = self.unknown_ids.into_iter().collect();
        stats.marker_ok = stats.marker_count == 1;
        stats.windows = windows.len() as u64;
        stats.tabs = tabs.len() as u64;
        stats.groups = groups.len() as u64;
        stats.dropped_tabs =
            dropped.no_navigations + dropped.window_missing + dropped.window_closed;
        stats.dropped_tab_reasons = dropped;
        Parsed {
            stats,
            windows,
            groups,
            tabs,
        }
    }

    /// Every tab with a real window and a usable navigation, in id order,
    /// plus the window each group was first seen in and what was dropped.
    fn resolve_tabs(
        &mut self,
        window_numbers: &HashMap<i32, u32>,
    ) -> (Vec<Tab>, BTreeMap<Token, u32>, DroppedTabs) {
        let mut dropped = DroppedTabs::default();
        let mut tabs = Vec::new();
        let mut group_windows: BTreeMap<Token, u32> = BTreeMap::new();
        for (&tab_id, state) in &self.tabs {
            let Some(window_id) = state.window_id else {
                dropped.window_missing += 1;
                continue;
            };
            let Some(&window) = window_numbers.get(&window_id) else {
                if self.closed_windows.contains(&window_id) {
                    dropped.window_closed += 1;
                } else {
                    dropped.window_missing += 1;
                }
                continue;
            };
            let Some((navigation, exact)) = resolve_navigation(state) else {
                dropped.no_navigations += 1;
                continue;
            };
            if !exact {
                self.stats.navigation_fallbacks += 1;
            }
            if let Some(token) = state.group {
                group_windows.entry(token).or_insert(window);
            }
            tabs.push(Tab {
                tab_id,
                window,
                position: state.position.unwrap_or(-1),
                url: navigation.url.clone(),
                title: navigation.title.clone(),
                pinned: state.pinned,
                active: false,
                group: state.group.map(|t| t.to_string()),
                last_active: state.last_active.and_then(chrome_time),
                window_id,
            });
        }
        (tabs, group_windows, dropped)
    }

    /// Windows in creation order, each told which of its (already sorted)
    /// tabs is in front. `SetSelectedTabInIndex` is a visual index, clamped
    /// as `SortTabsBasedOnVisualOrderAndClear` clamps it.
    fn resolve_windows(&self, window_numbers: &HashMap<i32, u32>, tabs: &mut [Tab]) -> Vec<Window> {
        let mut windows: Vec<Window> = self
            .windows
            .iter()
            .map(|(&id, &kind_id)| Window {
                id,
                number: window_numbers[&id],
                kind: window_kind(kind_id).to_owned(),
                kind_id,
                active_tab: None,
                tabs: 0,
            })
            .collect();
        for window in &mut windows {
            let members: Vec<usize> = (0..tabs.len())
                .filter(|&i| tabs[i].window == window.number)
                .collect();
            window.tabs = u32::try_from(members.len()).unwrap_or(u32::MAX);
            let Some(&selected) = self.selected_tab.get(&window.id) else {
                continue;
            };
            let Some(last) = members.len().checked_sub(1) else {
                continue;
            };
            let chosen = members[usize::try_from(selected).unwrap_or(0).min(last)];
            tabs[chosen].active = true;
            window.active_tab = Some(tabs[chosen].tab_id);
        }
        windows
    }

    /// Only groups that still have a tab; a token without a metadata record
    /// is emitted bare, as `AddTabsToWindows` does.
    fn resolve_groups(&mut self, group_windows: BTreeMap<Token, u32>) -> Vec<Group> {
        group_windows
            .into_iter()
            .map(|(token, window)| {
                if let Some(meta) = self.group_meta.get(&token) {
                    Group {
                        id: token.to_string(),
                        title: Some(meta.title.clone()),
                        colour: colour_name(meta.colour).to_owned(),
                        colour_id: Some(meta.colour),
                        collapsed: meta.collapsed,
                        saved_guid: meta.saved_guid.clone(),
                        window,
                    }
                } else {
                    self.stats.groups_without_metadata += 1;
                    Group {
                        id: token.to_string(),
                        title: None,
                        colour: colour_name(u32::MAX).to_owned(),
                        colour_id: None,
                        collapsed: false,
                        saved_guid: None,
                        window,
                    }
                }
            })
            .collect()
    }
}

impl TabState {
    /// `ProcessTabNavigationPathPrunedCommand`: fix the selected index, erase
    /// `[index, index + count)`, renumber what survives.
    fn prune(&mut self, index: i32, count: i32) {
        let start = i64::from(index);
        let end = start + i64::from(count);
        if let Some(cur) = self.selected {
            let cur = i64::from(cur);
            let fixed = if cur >= start && cur < end {
                start - 1
            } else if cur >= end {
                cur - i64::from(count)
            } else {
                cur
            };
            self.selected = i32::try_from(fixed).ok();
        }
        let survivors = std::mem::take(&mut self.navigations);
        for (i, nav) in survivors {
            let i = i64::from(i);
            if i >= start && i < end {
                continue;
            }
            let renumbered = if i >= start { i - i64::from(count) } else { i };
            if let Ok(key) = i32::try_from(renumbered) {
                self.navigations.insert(key, nav);
            }
        }
    }
}

/// `AddTabsToWindows`: the first navigation with `index >= selected`, else
/// the last one. The boolean says whether the match was exact.
fn resolve_navigation(state: &TabState) -> Option<(&Navigation, bool)> {
    let selected = state.selected.unwrap_or(-1);
    if let Some(nav) = state.navigations.get(&selected) {
        return Some((nav, true));
    }
    let nearest = state
        .navigations
        .range(selected..)
        .next()
        .or_else(|| state.navigations.iter().next_back());
    nearest.map(|(_, nav)| (nav, false))
}

fn pair(contents: &[u8]) -> Option<(i32, i32)> {
    i32_at(contents, 0).zip(i32_at(contents, 4))
}

/// `SessionWindow::WindowType`.
fn window_kind(kind: i32) -> &'static str {
    match kind {
        0 => "normal",
        1 => "popup",
        2 => "app",
        3 => "devtools",
        4 => "app_popup",
        _ => "unknown",
    }
}

/// `tab_groups::TabGroupColorId`; the header says unknown values are grey.
pub fn colour_name(colour: u32) -> &'static str {
    match colour {
        1 => "blue",
        2 => "red",
        3 => "yellow",
        4 => "green",
        5 => "pink",
        6 => "purple",
        7 => "cyan",
        8 => "orange",
        _ => "grey",
    }
}

#[cfg(test)]
#[path = "../tests/common/session_builder.rs"]
mod session_builder;

#[cfg(test)]
mod tests {
    use super::session_builder::SessionBuilder;
    use super::*;

    fn urls(parsed: &Parsed) -> Vec<&str> {
        parsed.tabs.iter().map(|t| t.url.as_str()).collect()
    }

    #[test]
    fn command_table_is_chosen_by_file_name_only() {
        assert_eq!(
            CommandTable::for_file_name("Session_13434341530659553"),
            Some(CommandTable::Session)
        );
        assert_eq!(
            CommandTable::for_file_name("Apps_1"),
            Some(CommandTable::Session)
        );
        assert_eq!(
            CommandTable::for_file_name("Tabs_13434341530659553"),
            Some(CommandTable::Tabs)
        );
        assert_eq!(CommandTable::for_file_name("session.snss"), None);
    }

    #[test]
    fn one_window_one_tab() {
        let bytes = SessionBuilder::new()
            .simple_tab(1, 2, "https://example.test/a", "A")
            .marker()
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.stats.tabs, 1);
        assert_eq!(parsed.stats.windows, 1);
        assert_eq!(parsed.stats.commands, 6);
        assert!(parsed.stats.marker_ok);
        assert!(!parsed.stats.is_degraded());
        let tab = &parsed.tabs[0];
        assert_eq!((tab.tab_id, tab.window, tab.position), (2, 1, 0));
        assert_eq!(tab.url, "https://example.test/a");
        assert_eq!(tab.title, "A");
        assert_eq!(parsed.windows[0].kind, "normal");
        assert_eq!(parsed.stats.commands_by_id[&6], 1);
    }

    #[test]
    fn unknown_commands_are_counted_not_fatal() {
        let bytes = SessionBuilder::new()
            .raw_command(200, &[1, 2, 3])
            .simple_tab(1, 2, "https://example.test/a", "A")
            .raw_command(3, &[])
            .raw_command(200, &[])
            .marker()
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.stats.tabs, 1);
        assert_eq!(parsed.stats.unknown_commands, 3);
        assert_eq!(parsed.stats.unknown_command_ids, vec![3, 200]);
        assert!(parsed.stats.is_degraded());
    }

    #[test]
    fn obsolete_but_chromium_known_ids_are_not_unknown() {
        let bytes = SessionBuilder::new()
            .raw_command(15, &[0; 12])
            .raw_command(1, &[])
            .simple_tab(1, 2, "https://example.test/a", "A")
            .marker()
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.stats.unknown_commands, 0);
        assert_eq!(parsed.stats.tabs, 1);
    }

    #[test]
    fn every_truncation_point_is_nonfatal() {
        let bytes = SessionBuilder::new()
            .simple_tab(1, 2, "https://example.test/a", "A")
            .simple_tab(1, 3, "https://example.test/b", "B")
            .marker()
            .build();
        let full = parse(&bytes).unwrap();
        assert_eq!(full.stats.tabs, 2);
        for cut in snss::HEADER_LEN..=bytes.len() {
            let parsed = parse(&bytes[..cut]).unwrap_or_else(|e| panic!("cut {cut}: {e}"));
            assert!(parsed.stats.tabs <= 2, "cut {cut}");
            assert!(parsed.stats.truncated_bytes <= cut as u64, "cut {cut}");
            assert_eq!(
                parsed.stats.truncated_bytes > 0,
                !command_boundaries(&bytes).contains(&cut),
                "cut {cut}"
            );
        }
        // One byte short: the marker is lost, both tabs survive.
        let torn = parse(&bytes[..bytes.len() - 1]).unwrap();
        assert_eq!(torn.stats.tabs, 2);
        assert_eq!(torn.stats.truncated_bytes, 2);
        assert!(!torn.stats.marker_ok);
        // Cut mid length-prefix of the second tab's first command.
        let boundaries = command_boundaries(&bytes);
        let second_tab_start = boundaries[6];
        let torn = parse(&bytes[..=second_tab_start]).unwrap();
        assert_eq!(torn.stats.tabs, 1);
        assert_eq!(torn.stats.truncated_bytes, 1);
        assert_eq!(torn.stats.commands, 6);
    }

    /// Offsets at which each command starts, plus the end of the buffer.
    fn command_boundaries(bytes: &[u8]) -> Vec<usize> {
        let mut offsets = Vec::new();
        let mut offset = snss::HEADER_LEN;
        while offset < bytes.len() {
            offsets.push(offset);
            let size = usize::from(snss::u16_at(bytes, offset).unwrap());
            offset += 2 + size;
        }
        offsets.push(bytes.len());
        offsets
    }

    #[test]
    fn selected_index_falls_back_to_nearest_then_last() {
        // Selected 5, navigations at 3 and 7: nearest >= 5 is 7.
        let bytes = SessionBuilder::new()
            .set_tab_window(1, 2)
            .set_tab_index(2, 0)
            .set_window_type(1, 0)
            .navigation(2, 3, "https://example.test/three", "3")
            .navigation(2, 7, "https://example.test/seven", "7")
            .select_navigation(2, 5)
            .marker()
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(urls(&parsed), vec!["https://example.test/seven"]);
        assert_eq!(parsed.stats.navigation_fallbacks, 1);
        assert_eq!(parsed.stats.dropped_tabs, 0);

        // Selected 9, nothing at or after: the last one.
        let bytes = SessionBuilder::new()
            .set_tab_window(1, 2)
            .set_tab_index(2, 0)
            .set_window_type(1, 0)
            .navigation(2, 3, "https://example.test/three", "3")
            .navigation(2, 7, "https://example.test/seven", "7")
            .select_navigation(2, 9)
            .marker()
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(urls(&parsed), vec!["https://example.test/seven"]);
        assert_eq!(parsed.stats.navigation_fallbacks, 1);
    }

    #[test]
    fn tab_without_navigations_is_dropped_and_counted() {
        let bytes = SessionBuilder::new()
            .set_tab_window(1, 2)
            .set_tab_index(2, 0)
            .set_window_type(1, 0)
            .select_navigation(2, 0)
            .simple_tab(1, 3, "https://example.test/b", "B")
            .marker()
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.stats.tabs, 1);
        assert_eq!(parsed.stats.dropped_tabs, 1);
        assert_eq!(parsed.stats.dropped_tab_reasons.no_navigations, 1);
    }

    #[test]
    fn tab_in_unknown_or_closed_window_is_dropped_and_counted() {
        let bytes = SessionBuilder::new()
            .simple_tab(1, 2, "https://example.test/a", "A")
            // Window 9 never gets SetWindowType.
            .set_tab_window(9, 3)
            .set_tab_index(3, 0)
            .navigation(3, 0, "https://example.test/b", "B")
            .select_navigation(3, 0)
            // Window 5 is real, then closed.
            .simple_tab(5, 4, "https://example.test/c", "C")
            .window_closed(5)
            // Tab 6 never gets a window at all.
            .navigation(6, 0, "https://example.test/d", "D")
            .marker()
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(urls(&parsed), vec!["https://example.test/a"]);
        assert_eq!(parsed.stats.windows, 1);
        assert_eq!(parsed.stats.dropped_tabs, 3);
        assert_eq!(parsed.stats.dropped_tab_reasons.window_missing, 2);
        assert_eq!(parsed.stats.dropped_tab_reasons.window_closed, 1);
    }

    #[test]
    fn tab_closed_erases_the_tab() {
        let bytes = SessionBuilder::new()
            .simple_tab(1, 2, "https://example.test/a", "A")
            .simple_tab(1, 3, "https://example.test/b", "B")
            .tab_closed(2)
            .marker()
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(urls(&parsed), vec!["https://example.test/b"]);
        assert_eq!(parsed.stats.dropped_tabs, 0);
    }

    #[test]
    fn malformed_payloads_are_counted_and_skipped() {
        let bytes = SessionBuilder::new()
            .raw_command(id::SET_TAB_WINDOW, &[1, 0, 0])
            .raw_command(id::UPDATE_TAB_NAVIGATION, &[9, 9, 9, 9, 9])
            .raw_command(id::NAV_PRUNED, &[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])
            .raw_command(id::NAV_PRUNED_FROM_FRONT, &[2, 0, 0, 0, 0, 0, 0, 0])
            .simple_tab(1, 2, "https://example.test/a", "A")
            .marker()
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.stats.tabs, 1);
        assert_eq!(parsed.stats.malformed_commands, 4);
        assert_eq!(parsed.stats.unknown_commands, 0);
    }

    #[test]
    fn navigation_with_missing_title_keeps_the_url() {
        let mut payload = Vec::new();
        payload.extend(2i32.to_le_bytes());
        payload.extend(0i32.to_le_bytes());
        payload.extend(4i32.to_le_bytes());
        payload.extend(b"a://");
        let mut contents = u32::try_from(payload.len()).unwrap().to_le_bytes().to_vec();
        contents.extend(payload);
        let bytes = SessionBuilder::new()
            .set_tab_window(1, 2)
            .set_tab_index(2, 0)
            .set_window_type(1, 0)
            .raw_command(id::UPDATE_TAB_NAVIGATION, &contents)
            .select_navigation(2, 0)
            .marker()
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(urls(&parsed), vec!["a://"]);
        assert_eq!(parsed.tabs[0].title, "");
        assert_eq!(parsed.stats.malformed_commands, 1);
    }

    #[test]
    fn prune_from_back_drops_forward_history_only() {
        let bytes = SessionBuilder::new()
            .set_tab_window(1, 2)
            .set_tab_index(2, 0)
            .set_window_type(1, 0)
            .navigation(2, 0, "https://example.test/0", "0")
            .navigation(2, 1, "https://example.test/1", "1")
            .navigation(2, 2, "https://example.test/2", "2")
            .select_navigation(2, 1)
            .prune_back(2, 2)
            .marker()
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(urls(&parsed), vec!["https://example.test/1"]);
        assert_eq!(parsed.stats.navigation_fallbacks, 0);
    }

    #[test]
    fn prune_from_front_shifts_indices_and_selection() {
        let bytes = SessionBuilder::new()
            .set_tab_window(1, 2)
            .set_tab_index(2, 0)
            .set_window_type(1, 0)
            .navigation(2, 0, "https://example.test/0", "0")
            .navigation(2, 1, "https://example.test/1", "1")
            .navigation(2, 2, "https://example.test/2", "2")
            .select_navigation(2, 2)
            .prune_front(2, 2)
            .marker()
            .build();
        let parsed = parse(&bytes).unwrap();
        // Entry 2 became entry 0 and selection followed it.
        assert_eq!(urls(&parsed), vec!["https://example.test/2"]);
        assert_eq!(parsed.stats.navigation_fallbacks, 0);

        // Selection inside the pruned range becomes -1 and falls back.
        let bytes = SessionBuilder::new()
            .set_tab_window(1, 2)
            .set_tab_index(2, 0)
            .set_window_type(1, 0)
            .navigation(2, 0, "https://example.test/0", "0")
            .navigation(2, 1, "https://example.test/1", "1")
            .navigation(2, 2, "https://example.test/2", "2")
            .select_navigation(2, 1)
            .prune_front(2, 2)
            .marker()
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(urls(&parsed), vec!["https://example.test/2"]);
        assert_eq!(parsed.stats.navigation_fallbacks, 1);
    }

    #[test]
    fn prune_range_in_the_middle() {
        let bytes = SessionBuilder::new()
            .set_tab_window(1, 2)
            .set_tab_index(2, 0)
            .set_window_type(1, 0)
            .navigation(2, 0, "https://example.test/0", "0")
            .navigation(2, 1, "https://example.test/1", "1")
            .navigation(2, 2, "https://example.test/2", "2")
            .navigation(2, 3, "https://example.test/3", "3")
            .select_navigation(2, 3)
            .prune(2, 1, 2)
            .marker()
            .build();
        let parsed = parse(&bytes).unwrap();
        // 1 and 2 gone; 3 renumbered to 1; selection 3 -> 1.
        assert_eq!(urls(&parsed), vec!["https://example.test/3"]);
        assert_eq!(parsed.stats.navigation_fallbacks, 0);

        // Selection before the range is untouched.
        let bytes = SessionBuilder::new()
            .set_tab_window(1, 2)
            .set_tab_index(2, 0)
            .set_window_type(1, 0)
            .navigation(2, 0, "https://example.test/0", "0")
            .navigation(2, 1, "https://example.test/1", "1")
            .navigation(2, 2, "https://example.test/2", "2")
            .select_navigation(2, 0)
            .prune(2, 1, 2)
            .marker()
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(urls(&parsed), vec!["https://example.test/0"]);
        assert_eq!(parsed.stats.navigation_fallbacks, 0);
    }

    #[test]
    fn groups_carry_title_colour_and_collapsed_state() {
        let token = (0x1122_3344_5566_7788, 0x99AA_BBCC_DDEE_FF00);
        let bytes = SessionBuilder::new()
            .group_metadata(token, "Work", 8, true, Some("guid-1"))
            .simple_tab(1, 2, "https://example.test/a", "A")
            .simple_tab(1, 3, "https://example.test/b", "B")
            .simple_tab(1, 4, "https://example.test/c", "C")
            .set_tab_group(2, Some(token))
            .set_tab_group(3, Some(token))
            .set_tab_group(3, None)
            .marker()
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.groups.len(), 1);
        let group = &parsed.groups[0];
        assert_eq!(group.id, "112233445566778899AABBCCDDEEFF00");
        assert_eq!(group.title.as_deref(), Some("Work"));
        assert_eq!(group.colour, "orange");
        assert_eq!(group.colour_id, Some(8));
        assert!(group.collapsed);
        assert_eq!(group.saved_guid.as_deref(), Some("guid-1"));
        assert_eq!(group.window, 1);
        let by_id: Vec<Option<&str>> = parsed.tabs.iter().map(|t| t.group.as_deref()).collect();
        assert_eq!(by_id, vec![Some(group.id.as_str()), None, None]);
        assert_eq!(parsed.stats.groups, 1);
        assert_eq!(parsed.stats.groups_without_metadata, 0);
    }

    #[test]
    fn group_without_metadata_is_emitted_grey_and_dangling_groups_are_dropped() {
        let orphan = (1, 2);
        let dangling = (3, 4);
        let bytes = SessionBuilder::new()
            .group_metadata(dangling, "Gone", 1, false, None)
            .simple_tab(1, 2, "https://example.test/a", "A")
            .set_tab_group(2, Some(orphan))
            .marker()
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.groups.len(), 1);
        assert_eq!(parsed.groups[0].id, "00000000000000010000000000000002");
        assert_eq!(parsed.groups[0].title, None);
        assert_eq!(parsed.groups[0].colour, "grey");
        assert_eq!(parsed.stats.groups_without_metadata, 1);
    }

    #[test]
    fn short_group_metadata_keeps_what_it_has() {
        // Pre-M88 shape: token, title, colour, nothing else. Unknown colour.
        let mut payload = Vec::new();
        payload.extend(5u64.to_le_bytes());
        payload.extend(6u64.to_le_bytes());
        payload.extend(1i32.to_le_bytes());
        payload.extend([b'T', 0, 0, 0]);
        payload.extend(77u32.to_le_bytes());
        let mut contents = u32::try_from(payload.len()).unwrap().to_le_bytes().to_vec();
        contents.extend(payload);
        let bytes = SessionBuilder::new()
            .raw_command(id::SET_TAB_GROUP_METADATA2, &contents)
            .simple_tab(1, 2, "https://example.test/a", "A")
            .set_tab_group(2, Some((5, 6)))
            .marker()
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.groups[0].title.as_deref(), Some("T"));
        assert_eq!(parsed.groups[0].colour, "grey");
        assert_eq!(parsed.groups[0].colour_id, Some(77));
        assert!(!parsed.groups[0].collapsed);
        assert_eq!(parsed.stats.malformed_commands, 0);
    }

    #[test]
    fn last_active_time_is_converted_from_chrome_epoch() {
        // 2026-09-05T04:36:42Z per the research doc's worked example.
        let bytes = SessionBuilder::new()
            .simple_tab(1, 2, "https://example.test/a", "A")
            .last_active(2, 13_433_056_602_603_321)
            .simple_tab(1, 3, "https://example.test/b", "B")
            .last_active(3, 0)
            .marker()
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(
            parsed.tabs[0].last_active.map(|t| t.to_string()),
            Some("2026-09-05T04:36:42.603321Z".to_owned())
        );
        assert_eq!(parsed.tabs[1].last_active, None);
    }

    #[test]
    fn pinned_active_and_ordering() {
        let bytes = SessionBuilder::new()
            .set_window_type(3, 0)
            .set_tab_window(3, 10)
            .set_tab_index(10, 1)
            .navigation(10, 0, "https://example.test/second", "")
            .select_navigation(10, 0)
            .set_tab_window(3, 11)
            .set_tab_index(11, 0)
            .navigation(11, 0, "https://example.test/first", "")
            .select_navigation(11, 0)
            .pinned(11, true)
            .selected_tab(3, 1)
            .simple_tab(7, 12, "https://example.test/popup", "")
            .set_window_type(7, 1)
            .selected_tab(7, 99)
            .marker()
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(
            urls(&parsed),
            vec![
                "https://example.test/first",
                "https://example.test/second",
                "https://example.test/popup"
            ]
        );
        assert!(parsed.tabs[0].pinned);
        assert!(!parsed.tabs[0].active);
        assert!(parsed.tabs[1].active);
        assert!(parsed.tabs[2].active);
        assert_eq!(parsed.windows[0].id, 3);
        assert_eq!(parsed.windows[0].number, 1);
        assert_eq!(parsed.windows[0].active_tab, Some(10));
        assert_eq!(parsed.windows[0].tabs, 2);
        assert_eq!(parsed.windows[1].kind, "popup");
        assert_eq!(parsed.windows[1].active_tab, Some(12));
    }

    #[test]
    fn missing_marker_and_double_marker_are_flagged_not_fatal() {
        let none = SessionBuilder::new()
            .simple_tab(1, 2, "https://example.test/a", "A")
            .build();
        let parsed = parse(&none).unwrap();
        assert_eq!(parsed.stats.tabs, 1);
        assert!(!parsed.stats.marker_ok);
        let twice = SessionBuilder::new()
            .marker()
            .simple_tab(1, 2, "https://example.test/a", "A")
            .marker()
            .build();
        let parsed = parse(&twice).unwrap();
        assert_eq!(parsed.stats.marker_count, 2);
        assert!(!parsed.stats.marker_ok);
    }

    #[test]
    fn header_errors_are_the_only_fatal_ones() {
        assert!(matches!(
            parse(b"SNSS\x05\0\0\0"),
            Err(HeaderError::Encrypted(5))
        ));
        assert!(matches!(parse(b"nope"), Err(HeaderError::TooShort(4))));
        assert!(parse(b"SNSS\x03\0\0\0").is_ok());
    }

    #[test]
    fn chrome_time_edges() {
        assert_eq!(chrome_time(0), None);
        assert_eq!(chrome_time(-5), None);
        assert_eq!(
            chrome_time(WINDOWS_EPOCH_OFFSET_US).map(Timestamp::as_second),
            Some(0)
        );
        assert_eq!(chrome_time(i64::MAX), None);
    }
}

#[cfg(test)]
mod properties {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// No panic, and every counter is bounded by the input.
        #[test]
        fn arbitrary_bodies_never_panic(body in proptest::collection::vec(any::<u8>(), 0..2048)) {
            let mut bytes = snss::MAGIC.to_vec();
            bytes.extend(snss::VERSION_CLEARTEXT.to_le_bytes());
            bytes.extend(&body);
            let parsed = parse(&bytes).unwrap();
            prop_assert!(parsed.stats.truncated_bytes <= body.len() as u64);
            prop_assert!(parsed.stats.commands <= body.len() as u64 / 3 + 1);
            prop_assert!(parsed.stats.unknown_commands <= parsed.stats.commands);
            prop_assert!(parsed.stats.malformed_commands <= parsed.stats.commands);
            prop_assert!(parsed.stats.tabs + parsed.stats.dropped_tabs <= parsed.stats.commands);
        }

        #[test]
        fn arbitrary_headers_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..16)) {
            let _ = parse(&bytes);
        }
    }
}
