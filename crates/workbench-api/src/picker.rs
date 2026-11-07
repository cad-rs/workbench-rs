//! The in-app file picker (design.md §12.1).
//!
//! Storage without an OS dialog (the browser's origin-private file system,
//! cloud stores) is picked from inside the app. The runtime keeps this state
//! in [`crate::WorkspaceState::file_picker`] and lists folders into it; the
//! frontend draws it and reports the user's moves back.

use crate::storage::{has_extension, with_extension, Entry, Location};

pub struct FilePicker {
    /// The suggested file name for a save picker; None for an open picker.
    pub save: Option<String>,
    /// Files are filtered by these extensions (lowercase, no dot; empty
    /// shows every file).
    pub extensions: Vec<String>,
    /// The places to pick from, as (provider id, label).
    pub sources: Vec<(String, String)>,
    /// The place being browsed.
    pub provider: String,
    /// The folders from the place's root down to the one shown, as (key,
    /// name). Never empty: the root is `("", label)`.
    pub trail: Vec<(String, String)>,
    /// The folder shown: its folders, then the files that pass the filter,
    /// each by name. None while it is being listed.
    pub listing: Option<Result<Vec<Entry>, String>>,
}

impl FilePicker {
    /// A picker showing the root of the first source; None without sources.
    pub fn new(
        save: Option<String>,
        extensions: Vec<String>,
        sources: Vec<(String, String)>,
    ) -> Option<Self> {
        let first = sources.first()?.0.clone();
        let mut picker = Self {
            save,
            extensions,
            sources,
            provider: String::new(),
            trail: Vec::new(),
            listing: None,
        };
        picker.browse(&first);
        Some(picker)
    }

    /// The folder shown.
    pub fn dir(&self) -> Location {
        let key = self.trail.last().map(|(key, _)| key.as_str()).unwrap_or("");
        Location::new(self.provider.as_str(), key)
    }

    /// Show the root of the source `provider`.
    pub fn browse(&mut self, provider: &str) {
        let label = self
            .sources
            .iter()
            .find(|(id, _)| id == provider)
            .map(|(_, label)| label.clone())
            .unwrap_or_else(|| provider.to_string());
        self.provider = provider.to_string();
        self.trail = vec![(String::new(), label)];
        self.listing = None;
    }

    /// Show `folder`, an entry of the listing.
    pub fn enter(&mut self, folder: &Entry) {
        self.trail.push((folder.key.clone(), folder.name.clone()));
        self.listing = None;
    }

    /// Show the folder at `depth` in the trail (0 is the root).
    pub fn up_to(&mut self, depth: usize) {
        self.trail.truncate(depth + 1);
        self.listing = None;
    }

    /// Take a listing of `dir`. A listing of a folder no longer shown is
    /// dropped (the user moved on while it ran).
    pub fn listed(&mut self, dir: &Location, listing: Result<Vec<Entry>, String>) {
        if *dir != self.dir() {
            return;
        }
        self.listing = Some(listing.map(|mut entries| {
            entries.retain(|entry| entry.is_dir || self.accepts(&entry.name));
            entries.sort_by_cached_key(|entry| (!entry.is_dir, entry.name.to_lowercase()));
            entries
        }));
    }

    /// The file name a save writes for the typed `name`: the first
    /// extension is added when the name has none of the accepted ones.
    pub fn file_name(&self, name: &str) -> String {
        with_extension(name, &self.extensions)
    }

    /// Where a save of the typed `name` goes: in the folder shown, with an
    /// accepted extension. None for a blank name.
    pub fn target(&self, name: &str) -> Option<Location> {
        (!name.trim().is_empty()).then(|| self.dir().child(&self.file_name(name)))
    }

    /// The listed file called `name`, if any (a save over it overwrites).
    pub fn existing(&self, name: &str) -> Option<&Entry> {
        let entries = self.listing.as_ref()?.as_ref().ok()?;
        entries.iter().find(|entry| !entry.is_dir && entry.name == name)
    }

    fn accepts(&self, name: &str) -> bool {
        has_extension(name, &self.extensions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool) -> Entry {
        Entry { key: format!("docs/{name}"), name: name.into(), is_dir }
    }

    fn picker() -> FilePicker {
        let sources = vec![
            ("opfs".to_string(), "Browser storage".to_string()),
            ("cloud".to_string(), "Cloud drive".to_string()),
        ];
        FilePicker::new(Some("part.rcad".into()), vec!["rcad".into()], sources).unwrap()
    }

    #[test]
    fn listings_put_folders_first_and_filter_files() {
        let mut picker = picker();
        assert_eq!(picker.dir(), Location::new("opfs", ""));
        let listing = vec![
            entry("b.rcad", false),
            entry("notes.txt", false),
            entry("Z", true),
            entry("A.RCAD", false),
            entry("a", true),
        ];
        picker.listed(&Location::new("opfs", ""), Ok(listing));
        let names: Vec<_> = picker.listing.unwrap().unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["a", "Z", "A.RCAD", "b.rcad"]);
    }

    #[test]
    fn a_listing_of_a_folder_no_longer_shown_is_dropped() {
        let mut picker = picker();
        picker.enter(&entry("sub", true));
        picker.listed(&Location::new("opfs", ""), Ok(vec![]));
        assert!(picker.listing.is_none());
        assert_eq!(picker.dir(), Location::new("opfs", "docs/sub"));

        picker.up_to(0);
        assert_eq!(picker.trail, [(String::new(), "Browser storage".to_string())]);
        picker.browse("cloud");
        assert_eq!(picker.trail[0].1, "Cloud drive");
    }

    #[test]
    fn save_names_get_an_accepted_extension() {
        let mut picker = picker();
        assert_eq!(picker.file_name(" part "), "part.rcad");
        assert_eq!(picker.file_name("part.RCAD"), "part.RCAD");
        assert_eq!(picker.file_name("part.v2"), "part.v2.rcad");
        picker.listed(&picker.dir(), Ok(vec![entry("part.rcad", false)]));
        assert!(picker.existing("part.rcad").is_some());
        assert!(picker.existing("other.rcad").is_none());

        assert_eq!(picker.target("  "), None);
        picker.enter(&entry("sub", true));
        assert_eq!(picker.target("part"), Some(Location::new("opfs", "docs/sub/part.rcad")));
    }
}
