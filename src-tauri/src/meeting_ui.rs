//! Speaker names from the meeting app itself, the way Granola's "speaker tags" work: Teams, Zoom and
//! Google Meet show participant names and highlight whoever is talking, and Windows UI Automation
//! (the screen-reader interface) can read that text. Names and timings only; nothing is captured.

use crate::speakers::Span;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default, Clone)]
pub struct Shared {
    pub timeline: Arc<Mutex<Vec<Span>>>,
    pub participants: Arc<Mutex<Vec<String>>>,
    /// Meeting app the names came from ("Teams", "Zoom", "Google Meet"); empty until one is seen.
    pub app: Arc<Mutex<String>>,
}

#[derive(Serialize, Default)]
pub struct Snapshot {
    pub app: String,
    pub window: String,
    pub elements: usize,
    pub participants: Vec<String>,
    pub speaking: Vec<String>,
    pub dump: String,
}

/// Which meeting app a top-level window title belongs to.
fn app_of(title: &str) -> Option<&'static str> {
    let t = title.to_lowercase();
    if t.contains("microsoft teams") { Some("Teams") }
    else if t.contains("zoom meeting") || t.contains("zoom webinar") { Some("Zoom") }
    else if t.starts_with("meet - ") || t.starts_with("meet – ") || t.contains("google meet") { Some("Google Meet") }
    else { None }
}

const SPEAKING: [&str; 5] = ["is speaking", "is talking", "active speaker", "speaking", "talking"];
const ROSTER: [&str; 8] = ["muted", "unmuted", "microphone", "(guest)", "organizer", "presenter", "attendee", "camera"];
const NOT_A_NAME: [&str; 25] = [
    "mute", "camera", "microphone", "share", "leave", "chat", "people", "raise", "react", "more", "view", "meeting", "record", "settings",
    "show", "only", "layout", "gallery", "speaker", "speakers", "together", "focus", "pin", "spotlight", "options",
];
/// Control types that are app commands ("Show only active speaker" menu item), never a participant.
const COMMANDS: [&str; 6] = ["menu item", "radio button", "check box", "tab item", "combo box", "toggle"];

/// The person a tile or roster label is about: the text before the first comma / " is " / "(".
fn person(label: &str) -> Option<String> {
    let cut = [",", " is ", " (", " - ", "\n"].iter().filter_map(|s| label.find(s)).min().unwrap_or(label.len());
    let n = label[..cut].trim();
    let l = n.to_lowercase();
    let ok = (2..=60).contains(&n.chars().count())
        && n.chars().any(char::is_alphabetic)
        && !["you", "me"].contains(&l.as_str())
        && !NOT_A_NAME.iter().any(|w| l.split_whitespace().any(|x| x == *w))
        && !SPEAKING.iter().any(|m| l.contains(m));
    ok.then(|| n.to_string())
}

/// Speaking names and participant names found in one read of the window's labels.
fn parse(labels: &[(String, String)]) -> (Vec<String>, Vec<String>) {
    let (mut speaking, mut people) = (vec![], vec![]);
    for (name, extra) in labels {
        let all = format!("{name} {extra}").to_lowercase();
        if COMMANDS.iter().any(|c| extra.to_lowercase().ends_with(c)) { continue; }
        let Some(p) = person(name) else { continue };
        // The user's own tile ("(You)") is skipped: their speech is on the mic track already.
        let me = all.contains("(you)") || all.contains("(me)");
        if !me && SPEAKING.iter().any(|m| all.contains(m)) && !all.contains("not speaking") && !speaking.contains(&p) { speaking.push(p.clone()); }
        if ROSTER.iter().any(|m| all.contains(m)) && !people.contains(&p) { people.push(p); }
    }
    (speaking, people)
}

/// Records who the meeting app shows as speaking, about once a second, until `stop`.
pub fn watch(stop: Arc<AtomicBool>, t0: Instant, shared: Shared) {
    std::thread::spawn(move || {
        let Some(ui) = imp::Reader::new() else { return };
        while !stop.load(Relaxed) {
            let tick = Instant::now();
            if let Some((app, _, labels)) = ui.read() {
                let (speaking, people) = parse(&labels);
                *shared.app.lock().unwrap() = app.into();
                let now = t0.elapsed().as_millis() as u64;
                let mut tl = shared.timeline.lock().unwrap();
                for name in speaking {
                    // Extend the name's open stretch if it was speaking at the previous read, else start one.
                    match tl.iter_mut().rev().find(|s| s.name == name && now - s.end_ms <= 2000) {
                        Some(s) => s.end_ms = now,
                        None => tl.push(Span { start_ms: now.saturating_sub(500), end_ms: now, name }),
                    }
                }
                let mut ps = shared.participants.lock().unwrap();
                for p in people { if !ps.contains(&p) { ps.push(p); } }
            }
            std::thread::sleep(Duration::from_millis(1000).saturating_sub(tick.elapsed()));
        }
    });
}

/// One read for the "Check speaker names" button; the full label list goes to `dump` so new app
/// layouts can be taught to `parse`.
pub fn snapshot() -> Result<Snapshot, String> {
    let ui = imp::Reader::new().ok_or("Windows UI Automation isn't available")?;
    let (app, window, labels) = ui.read().ok_or("No Teams, Zoom or Google Meet window is open")?;
    let (speaking, participants) = parse(&labels);
    let dump = labels.iter().filter(|l| !l.0.is_empty()).map(|(n, x)| format!("{n}\t{x}")).collect::<Vec<_>>().join("\n");
    Ok(Snapshot { app: app.into(), window, elements: labels.len(), participants, speaking, dump })
}

#[cfg(windows)]
mod imp {
    use super::app_of;
    use uiautomation::types::{TreeScope, UIProperty};
    use uiautomation::UIAutomation;

    pub struct Reader(UIAutomation);

    impl Reader {
        pub fn new() -> Option<Self> {
            UIAutomation::new().ok().map(Self)
        }

        /// (app, window title, [(name, help text + status + control type)]) of the first meeting window.
        pub fn read(&self) -> Option<(&'static str, String, Vec<(String, String)>)> {
            let a = &self.0;
            let all = a.create_true_condition().ok()?;
            let window = a.get_root_element().ok()?.find_all(TreeScope::Children, &all).ok()?.into_iter()
                .find(|w| w.get_name().ok().as_deref().and_then(app_of).is_some())?;
            let title = window.get_name().ok()?;
            let req = a.create_cache_request().ok()?;
            for p in [UIProperty::Name, UIProperty::HelpText, UIProperty::ItemStatus, UIProperty::LocalizedControlType] {
                req.add_property(p).ok()?;
            }
            // ponytail: full descendant read each second; switch to property-changed events if big calls get slow.
            let els = window.find_all_build_cache(TreeScope::Descendants, &all, &req).ok()?;
            let labels = els.iter().map(|e| (
                e.get_cached_name().unwrap_or_default(),
                format!("{} {} {}", e.get_cached_help_text().unwrap_or_default(), e.get_cached_item_status().unwrap_or_default(),
                    e.get_cached_localized_control_type().unwrap_or_default()),
            )).collect();
            Some((app_of(&title)?, title, labels))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    // ponytail: macOS needs the Accessibility (AX) API version of this reader; until then names come from the AI.
    pub struct Reader;
    impl Reader {
        pub fn new() -> Option<Self> { None }
        pub fn read(&self) -> Option<(&'static str, String, Vec<(String, String)>)> { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tiles_and_roster() {
        let l = |n: &str, x: &str| (n.to_string(), x.to_string());
        let (speaking, people) = parse(&[
            l("Saurav Ashok, Speaking", "list item"),
            l("Anand Rajan, Muted", "list item"),
            l("Mute microphone", "button"),
            l("Akash Gunasekar (You), Unmuted", "list item"),
            l("Priya is talking", ""),
            l("Leave", "button"),
            l("Show only active speaker", "  menu item"),
            l("Show only active speaker", ""),
            l("Layout", "Change layout, camera  button"),
        ]);
        assert_eq!(speaking, ["Saurav Ashok", "Priya"]);
        assert_eq!(people, ["Anand Rajan", "Akash Gunasekar"]);
        assert_eq!(app_of("Weekly sync | Microsoft Teams"), Some("Teams"));
        assert_eq!(app_of("Meet - abc-defg-hij - Google Chrome"), Some("Google Meet"));
        assert_eq!(app_of("Inbox - Outlook"), None);
    }
}
