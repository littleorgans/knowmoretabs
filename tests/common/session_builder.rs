//! Builds synthetic SNSS bytes for tests, from the layouts in
//! `docs/research/session-format.md`. Every URL and title is invented;
//! nothing here ever comes from a real profile.
//!
//! Shared by the parser's unit tests (via `#[path]`) and the integration
//! tests, so the pruning commands that never occur in real files still get
//! exercised against bytes built the way Chrome would write them.

#![allow(dead_code)]

/// Appends commands in the order they are called, exactly as Chrome would.
#[derive(Debug, Clone)]
pub struct SessionBuilder {
    bytes: Vec<u8>,
}

impl Default for SessionBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionBuilder {
    /// A version-3 header and nothing else.
    pub fn new() -> Self {
        Self::with_version(3)
    }

    pub fn with_version(version: u32) -> Self {
        let mut bytes = b"SNSS".to_vec();
        bytes.extend(version.to_le_bytes());
        Self { bytes }
    }

    pub fn build(&self) -> Vec<u8> {
        self.bytes.clone()
    }

    /// `[size u16][id u8][contents]`, size counting the id byte.
    pub fn raw_command(mut self, id: u8, contents: &[u8]) -> Self {
        let size = u16::try_from(contents.len() + 1).expect("command fits in u16");
        self.bytes.extend(size.to_le_bytes());
        self.bytes.push(id);
        self.bytes.extend_from_slice(contents);
        self
    }

    pub fn raw_bytes(mut self, bytes: &[u8]) -> Self {
        self.bytes.extend_from_slice(bytes);
        self
    }

    fn pair(self, id: u8, a: i32, b: i32) -> Self {
        let mut contents = a.to_le_bytes().to_vec();
        contents.extend(b.to_le_bytes());
        self.raw_command(id, &contents)
    }

    pub fn set_tab_window(self, window: i32, tab: i32) -> Self {
        self.pair(0, window, tab)
    }

    pub fn set_tab_index(self, tab: i32, index: i32) -> Self {
        self.pair(2, tab, index)
    }

    pub fn prune_back(self, tab: i32, index: i32) -> Self {
        self.pair(5, tab, index)
    }

    pub fn navigation(self, tab: i32, index: i32, url: &str, title: &str) -> Self {
        let mut p = PickleWriter::new();
        p.i32(tab);
        p.i32(index);
        p.string(url);
        p.string16(title);
        // A short page-state blob and transition type, as real records have,
        // so the parser is proven to stop after the title.
        p.string("state");
        p.i32(0);
        self.raw_command(6, &p.finish())
    }

    pub fn select_navigation(self, tab: i32, index: i32) -> Self {
        self.pair(7, tab, index)
    }

    pub fn selected_tab(self, window: i32, index: i32) -> Self {
        self.pair(8, window, index)
    }

    pub fn set_window_type(self, window: i32, kind: i32) -> Self {
        self.pair(9, window, kind)
    }

    pub fn prune_front(self, tab: i32, count: i32) -> Self {
        self.pair(11, tab, count)
    }

    pub fn pinned(self, tab: i32, pinned: bool) -> Self {
        let mut contents = tab.to_le_bytes().to_vec();
        contents.extend([u8::from(pinned), 0, 0, 0]);
        self.raw_command(12, &contents)
    }

    pub fn tab_closed(self, tab: i32) -> Self {
        let mut contents = tab.to_le_bytes().to_vec();
        contents.extend([0; 12]);
        self.raw_command(16, &contents)
    }

    pub fn window_closed(self, window: i32) -> Self {
        let mut contents = window.to_le_bytes().to_vec();
        contents.extend([0; 12]);
        self.raw_command(17, &contents)
    }

    /// `micros` is Chrome's clock: microseconds since 1601-01-01 UTC.
    pub fn last_active(self, tab: i32, micros: i64) -> Self {
        let mut contents = tab.to_le_bytes().to_vec();
        contents.extend([0; 4]);
        contents.extend(micros.to_le_bytes());
        self.raw_command(21, &contents)
    }

    pub fn prune(self, tab: i32, index: i32, count: i32) -> Self {
        let mut contents = tab.to_le_bytes().to_vec();
        contents.extend(index.to_le_bytes());
        contents.extend(count.to_le_bytes());
        self.raw_command(24, &contents)
    }

    /// Command 25 with its C padding. `None` writes `has_group = false`.
    pub fn set_tab_group(self, tab: i32, token: Option<(u64, u64)>) -> Self {
        let (high, low) = token.unwrap_or((0, 0));
        let mut contents = tab.to_le_bytes().to_vec();
        contents.extend([0; 4]);
        contents.extend(high.to_le_bytes());
        contents.extend(low.to_le_bytes());
        contents.push(u8::from(token.is_some()));
        contents.extend([0; 7]);
        self.raw_command(25, &contents)
    }

    /// Command 27 in its current (M113+) shape.
    pub fn group_metadata(
        self,
        token: (u64, u64),
        title: &str,
        colour: u32,
        collapsed: bool,
        saved_guid: Option<&str>,
    ) -> Self {
        let mut p = PickleWriter::new();
        p.u64(token.0);
        p.u64(token.1);
        p.string16(title);
        p.u32(colour);
        p.bool(collapsed);
        p.bool(saved_guid.is_some());
        if let Some(guid) = saved_guid {
            p.string(guid);
        }
        self.raw_command(27, &p.finish())
    }

    pub fn marker(self) -> Self {
        self.raw_command(255, &[])
    }

    /// The six commands Chrome writes for one ordinary tab: window
    /// membership, position 0 unless the window already has tabs (callers
    /// pass distinct tab ids; position equals the number of earlier
    /// `simple_tab` calls for the same window), window type, one navigation
    /// at index 0, and selection of it.
    pub fn simple_tab(self, window: i32, tab: i32, url: &str, title: &str) -> Self {
        let position = self.count_tabs_in_window(window);
        self.set_tab_window(window, tab)
            .set_tab_index(tab, position)
            .set_window_type(window, 0)
            .navigation(tab, 0, url, title)
            .select_navigation(tab, 0)
    }

    fn count_tabs_in_window(&self, window: i32) -> i32 {
        let mut offset = 8;
        let mut count = 0;
        while offset + 2 <= self.bytes.len() {
            let size = usize::from(u16::from_le_bytes([
                self.bytes[offset],
                self.bytes[offset + 1],
            ]));
            if size == 0 || offset + 2 + size > self.bytes.len() {
                break;
            }
            let id = self.bytes[offset + 2];
            let contents = &self.bytes[offset + 3..offset + 2 + size];
            if id == 0 && contents.len() >= 4 {
                let w = i32::from_le_bytes([contents[0], contents[1], contents[2], contents[3]]);
                if w == window {
                    count += 1;
                }
            }
            offset += 2 + size;
        }
        count
    }
}

/// Writes a `base::Pickle` with Chromium's four-byte alignment rule.
#[derive(Debug, Default)]
pub struct PickleWriter {
    payload: Vec<u8>,
}

impl PickleWriter {
    pub fn new() -> Self {
        Self::default()
    }

    fn pad(&mut self) {
        while !self.payload.len().is_multiple_of(4) {
            self.payload.push(0);
        }
    }

    pub fn i32(&mut self, v: i32) {
        self.payload.extend(v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.payload.extend(v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.payload.extend(v.to_le_bytes());
    }

    pub fn bool(&mut self, v: bool) {
        self.payload.push(u8::from(v));
        self.pad();
    }

    pub fn string(&mut self, s: &str) {
        self.i32(i32::try_from(s.len()).expect("string fits"));
        self.payload.extend_from_slice(s.as_bytes());
        self.pad();
    }

    pub fn string16(&mut self, s: &str) {
        let units: Vec<u16> = s.encode_utf16().collect();
        self.i32(i32::try_from(units.len()).expect("string fits"));
        for unit in units {
            self.payload.extend(unit.to_le_bytes());
        }
        self.pad();
    }

    /// Prefixes the payload with its size, as `SessionCommand` stores it.
    pub fn finish(self) -> Vec<u8> {
        let mut contents = u32::try_from(self.payload.len())
            .expect("payload fits")
            .to_le_bytes()
            .to_vec();
        contents.extend(self.payload);
        contents
    }
}
