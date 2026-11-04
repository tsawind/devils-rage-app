//! Patch 63b: cycling through a client group with the foot pedal (or any hotkey), the way
//! EVE-O Preview does. A group (from launch.rs) has an ordered list of characters; a forward
//! or backward hotkey brings the next open client in that order to the front. Closed clients
//! are skipped. Nothing is sent to the game beyond bringing a window forward.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// A group member that stands for every client at the login / character-select screen
/// (EVE-O Preview's "EVE" entry). Each one is cycled like a character.
pub const LOGIN_SCREENS: &str = "Login screens";

/// The cycle order with "Login screens" expanded into one entry per open login window
/// ("login:<handle>", oldest handle first).
pub fn expand_login(order: &[String], logins: &[isize]) -> Vec<String> {
    let mut out = Vec::new();
    for name in order {
        if name == LOGIN_SCREENS {
            out.extend(logins.iter().map(|h| format!("login:{h}")));
        } else {
            out.push(name.clone());
        }
    }
    out
}

/// The window handle in an expanded "login:<handle>" entry.
pub fn login_handle(entry: &str) -> Option<isize> {
    entry.strip_prefix("login:")?.parse().ok()
}

/// Where each group is in its cycle: group id -> the character last brought forward.
static CURSOR: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// The next character to bring forward in `order`, skipping ones that aren't open, the way
/// EVE-O Preview does it: from the client in front (`current`) one step down the list (or up,
/// for backward), wrapping round. With no client of the group in front, forward starts at the
/// top of the list and backward at the bottom.
pub fn next_in_cycle(order: &[String], open: &HashSet<String>, current: Option<&str>, forward: bool) -> Option<String> {
    let live: Vec<&String> = order.iter().filter(|name| open.contains(*name)).collect();
    if live.is_empty() {
        return None;
    }
    let at = current.and_then(|c| live.iter().position(|name| name.as_str() == c));
    let next = match at {
        Some(i) if forward => (i + 1) % live.len(),
        Some(i) => (i + live.len() - 1) % live.len(),
        None if forward => 0,
        None => live.len() - 1,
    };
    Some(live[next].clone())
}

/// The group's next client from the one in front right now (`focused`), not from where the
/// pedal last went: clicking a client by hand and then pressing the pedal goes to its neighbour.
pub fn advance(group_id: &str, order: &[String], open: &HashSet<String>, focused: Option<&str>, forward: bool) -> Option<String> {
    let next = next_in_cycle(order, open, focused, forward)?;
    let mut guard = CURSOR.lock().unwrap();
    guard.get_or_insert_with(HashMap::new).insert(group_id.to_string(), next.clone());
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
    fn nothing_of_the_group_in_front_starts_at_the_top_or_the_bottom() {
        let order = list(&["Darkness Holder", "DeltaMechanic", "William Russ", "MyThai"]);
        let open = set(&["Darkness Holder", "DeltaMechanic", "William Russ", "MyThai"]);
        assert_eq!(next_in_cycle(&order, &open, None, true).as_deref(), Some("Darkness Holder"));
        assert_eq!(next_in_cycle(&order, &open, None, false).as_deref(), Some("MyThai"));
        assert_eq!(next_in_cycle(&order, &open, Some("Someone Else"), false).as_deref(), Some("MyThai"));
    }

    #[test]
    fn steps_from_the_client_in_front() {
        let order = list(&["Darkness Holder", "DeltaMechanic", "William Russ", "MyThai"]);
        let open = set(&["Darkness Holder", "DeltaMechanic", "William Russ", "MyThai"]);
        // Clicked William Russ by hand: next is MyThai, previous DeltaMechanic.
        assert_eq!(advance("g", &order, &open, Some("William Russ"), true).as_deref(), Some("MyThai"));
        assert_eq!(advance("g", &order, &open, Some("William Russ"), false).as_deref(), Some("DeltaMechanic"));
    }

    #[test]
    fn advance_moves_on_from_the_client_in_front() {
        forget("g");
        let order = list(&["A", "B", "C"]);
        let open = set(&["A", "B", "C"]);
        // Each press brings one forward; the next press starts from that one (now in front).
        let first = advance("g", &order, &open, None, true);
        assert_eq!(first.as_deref(), Some("A"));
        assert_eq!(advance("g", &order, &open, first.as_deref(), true).as_deref(), Some("B"));
        assert_eq!(advance("g", &order, &open, Some("B"), false).as_deref(), Some("A"));
        forget("g");
    }

    #[test]
    fn login_screens_expand_to_each_login_window() {
        let order = list(&["A", LOGIN_SCREENS, "B"]);
        assert_eq!(expand_login(&order, &[7, 9]), list(&["A", "login:7", "login:9", "B"]));
        assert_eq!(expand_login(&order, &[]), list(&["A", "B"]));
        assert_eq!(login_handle("login:9"), Some(9));
        assert_eq!(login_handle("Tsawind"), None);
    }
}
