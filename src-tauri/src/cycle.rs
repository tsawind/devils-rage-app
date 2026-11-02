//! Patch 63b: cycling through a client group with the foot pedal (or any hotkey), the way
//! EVE-O Preview does. A group (from launch.rs) has an ordered list of characters; a forward
//! or backward hotkey brings the next open client in that order to the front. Closed clients
//! are skipped. Nothing is sent to the game beyond bringing a window forward.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// Where each group is in its cycle: group id -> the character last brought forward.
static CURSOR: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// The next character to bring forward in `order`, skipping ones that aren't open. `current`
/// is the one brought forward last. Wraps around; starts at the first open one.
pub fn next_in_cycle(order: &[String], open: &HashSet<String>, current: Option<&str>, forward: bool) -> Option<String> {
    let live: Vec<&String> = order.iter().filter(|name| open.contains(*name)).collect();
    if live.is_empty() {
        return None;
    }
    let at = current.and_then(|c| live.iter().position(|name| name.as_str() == c));
    let next = match at {
        Some(i) if forward => (i + 1) % live.len(),
        Some(i) => (i + live.len() - 1) % live.len(),
        None => 0,
    };
    Some(live[next].clone())
}

/// Advance a group's cycle and return the character to focus (updating the stored cursor).
pub fn advance(group_id: &str, order: &[String], open: &HashSet<String>, forward: bool) -> Option<String> {
    let mut guard = CURSOR.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    let current = map.get(group_id).cloned();
    let next = next_in_cycle(order, open, current.as_deref(), forward)?;
    map.insert(group_id.to_string(), next.clone());
    Some(next)
}

/// Forget a group's place (e.g. when it's deleted or its members change).
pub fn forget(group_id: &str) {
    if let Some(map) = CURSOR.lock().unwrap().as_mut() {
        map.remove(group_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }
    fn list(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn cycles_forward_and_wraps() {
        let order = list(&["A", "B", "C"]);
        let open = set(&["A", "B", "C"]);
        assert_eq!(next_in_cycle(&order, &open, None, true).as_deref(), Some("A"));
        assert_eq!(next_in_cycle(&order, &open, Some("A"), true).as_deref(), Some("B"));
        assert_eq!(next_in_cycle(&order, &open, Some("C"), true).as_deref(), Some("A"));
    }

    #[test]
    fn cycles_backward() {
        let order = list(&["A", "B", "C"]);
        let open = set(&["A", "B", "C"]);
        assert_eq!(next_in_cycle(&order, &open, Some("A"), false).as_deref(), Some("C"));
        assert_eq!(next_in_cycle(&order, &open, Some("B"), false).as_deref(), Some("A"));
    }

    #[test]
    fn skips_closed_clients() {
        let order = list(&["A", "B", "C", "D"]);
        let open = set(&["A", "C"]); // B and D are closed
        assert_eq!(next_in_cycle(&order, &open, Some("A"), true).as_deref(), Some("C"));
        assert_eq!(next_in_cycle(&order, &open, Some("C"), true).as_deref(), Some("A"));
    }

    #[test]
    fn current_closed_starts_from_first_open() {
        let order = list(&["A", "B", "C"]);
        let open = set(&["B", "C"]); // A closed
        // current was A (now closed) -> fall back to first open
        assert_eq!(next_in_cycle(&order, &open, Some("A"), true).as_deref(), Some("B"));
    }

    #[test]
    fn nothing_open_gives_nothing() {
        assert_eq!(next_in_cycle(&list(&["A"]), &set(&[]), None, true), None);
    }

    #[test]
    fn advance_remembers_and_moves_on() {
        forget("g");
        let order = list(&["A", "B", "C"]);
        let open = set(&["A", "B", "C"]);
        assert_eq!(advance("g", &order, &open, true).as_deref(), Some("A"));
        assert_eq!(advance("g", &order, &open, true).as_deref(), Some("B"));
        assert_eq!(advance("g", &order, &open, false).as_deref(), Some("A"));
        forget("g");
    }
}
