//! What the message box's `@` list offers after the people:
//! `queryMentionResults` in 997625, as the chat box calls it (`e7` in
//! 496533). At most [`LIMIT`] rows in all: the roles fill what the people
//! leave, then `@everyone` and `@here`, in a server where we may mention
//! everyone.

use crate::{Guild, Role, Snowflake};

/// Rows in all (`limit`, 10).
pub const LIMIT: usize = 10;

/// `@everyone` or `@here`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Global {
    Everyone,
    Here,
}

impl Global {
    /// What the list matches the query against (`test`, 966382).
    fn test(self) -> &'static str {
        match self {
            Global::Everyone => "everyone",
            Global::Here => "here",
        }
    }
}

/// The roles offered for `query` once `listed` people are: mentionable ones,
/// or any where we may mention everyone, but `@everyone`'s own, whose names
/// hold the query's letters in order, best match first.
pub fn roles<'a>(guild: &'a Guild, query: &str, may_mention_everyone: bool, listed: usize) -> Vec<&'a Role> {
    let lower = query.to_lowercase();
    let mut sorted: Vec<&Role> = guild.roles.iter().collect();
    sorted.sort_by_key(|r| std::cmp::Reverse(r.position));
    let found: Vec<&Role> = sorted
        .into_iter()
        .filter(|r| {
            (r.mentionable || may_mention_everyone) && fuzzy(&lower, &r.name.to_lowercase()) && r.id != guild.id
        })
        .collect();
    let room = LIMIT.saturating_sub(listed);
    match_sorted(found, query, |r| r.name.as_str()).into_iter().take(room).collect()
}

/// `@everyone` and `@here` for `query`, where we may mention everyone in a
/// server, after `listed` rows: each while there is room and the query's
/// letters are in it in order.
pub fn globals(query: &str, may_mention_everyone: bool, private: bool, listed: usize) -> Vec<Global> {
    if private || !may_mention_everyone {
        return Vec::new();
    }
    let lower = query.to_lowercase();
    let mut out = Vec::new();
    for global in [Global::Everyone, Global::Here] {
        if listed + out.len() < LIMIT && fuzzy(&lower, global.test()) {
            out.push(global);
        }
    }
    out
}

/// Whether `user` may mention everyone in a channel: always in a private
/// one, else with Mention @everyone there, or while our membership waits
/// for screening (`canMentionEveryone` in 152210).
pub fn may_mention_everyone(private: bool, permitted: bool, pending: bool) -> bool {
    private || permitted || pending
}

/// Someone the list may offer, by the names a query is matched against.
#[derive(Clone, Debug)]
pub struct Person<'a> {
    pub id: Snowflake,
    pub username: &'a str,
    pub global_name: Option<&'a str>,
    pub nick: Option<&'a str>,
}

/// Who of `people` `query` finds, best first, at most `limit` (`eN` in
/// 997625): those with a name, nickname or global name starting with it,
/// then up to 50 more whose names hold its letters in order, to fill. Ties
/// keep `people`'s order. Names are lower-cased with their accents dropped;
/// the client also maps look-alike characters together, which is left out,
/// as in [`crate::switcher`].
pub fn rank_people(query: &str, people: &[Person], limit: usize) -> Vec<Snowflake> {
    let query = crate::switcher::fold(query);
    let mut starting = Vec::new();
    let mut fuzzy_found = Vec::new();
    for person in people {
        let names: Vec<String> = [Some(person.username), person.nick, person.global_name]
            .into_iter()
            .flatten()
            .map(crate::switcher::fold)
            .collect();
        if names.iter().any(|n| n.starts_with(&query)) {
            starting.push(person.id);
        } else if fuzzy_found.len() < 50 && names.iter().any(|n| fuzzy(&query, n)) {
            fuzzy_found.push(person.id);
        }
    }
    starting.extend(fuzzy_found);
    starting.truncate(limit);
    starting
}

/// `fuzzysearch` (91871): `needle`'s characters appear in `haystack` in order.
pub(crate) fn fuzzy(needle: &str, haystack: &str) -> bool {
    let mut hay = haystack.chars();
    needle.chars().all(|n| hay.any(|h| h == n))
}

/// How match-sorter ranks `value` for `query` (`u` in 896170), higher being
/// better; none below a fuzzy match.
fn rank(value: &str, query: &str) -> Option<f64> {
    let (value, query) = (plain(value), plain(query));
    if query.chars().count() > value.chars().count() {
        return None;
    }
    if value == query {
        return Some(7.0);
    }
    let (value, query) = (value.to_lowercase(), query.to_lowercase());
    if value == query {
        return Some(6.0);
    }
    if value.starts_with(&query) {
        return Some(5.0);
    }
    if value.contains(&format!(" {query}")) {
        return Some(4.0);
    }
    if value.contains(&query) {
        return Some(3.0);
    }
    if query.chars().count() == 1 {
        return None;
    }
    let acronym: String =
        value.split(' ').flat_map(|word| word.split('-')).filter_map(|part| part.chars().next()).collect();
    if acronym.contains(&query) {
        return Some(2.0);
    }
    // The query's letters in order: closer together and more of them is better.
    let value: Vec<char> = value.chars().collect();
    let mut matched = 0;
    let mut find = |c: char, from: usize| {
        let at = value[from..].iter().position(|&v| v == c)? + from;
        matched += 1;
        Some(at + 1)
    };
    let mut letters = query.chars();
    let first = find(letters.next()?, 0)?;
    let mut last = first;
    for c in letters {
        last = find(c, last)?;
    }
    let spread = (last - first) as f64;
    Some(1.0 + (1.0 / spread) * (f64::from(matched) / query.chars().count() as f64))
}

/// Without the accents match-sorter drops (`remove-accents`).
fn plain(text: &str) -> String {
    let decomposed = glib::normalize(text, glib::NormalizeMode::Default);
    let stripped: String = decomposed.chars().filter(|c| !('\u{300}'..='\u{36f}').contains(c)).collect();
    glib::normalize(stripped, glib::NormalizeMode::DefaultCompose).to_string()
}

/// `items` that `query` matches, best first and then by name, as
/// match-sorter sorts them.
fn match_sorted<T>(items: Vec<T>, query: &str, name: impl Fn(&T) -> &str) -> Vec<T> {
    let mut ranked: Vec<(f64, T)> =
        items.into_iter().filter_map(|item| Some((rank(name(&item), query)?, item))).collect();
    ranked.sort_by(|(a, x), (b, y)| b.total_cmp(a).then_with(|| name(x).cmp(name(y))));
    ranked.into_iter().map(|(_, item)| item).collect()
}

/// Roles' ids and names, for the list's rows.
pub fn named(roles: &[&Role]) -> Vec<(Snowflake, String)> {
    roles.iter().map(|r| (r.id, r.name.clone())).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn guild() -> Guild {
        serde_json::from_value(json!({
            "id": "1", "name": "G",
            "roles": [
                {"id": "1", "name": "@everyone", "position": 0, "mentionable": true},
                {"id": "2", "name": "Moderators", "position": 5, "mentionable": true},
                {"id": "3", "name": "Members", "position": 1, "mentionable": false},
                {"id": "4", "name": "Game Masters", "position": 3, "mentionable": true},
                {"id": "5", "name": "Mod", "position": 4, "mentionable": true},
            ],
        }))
        .unwrap()
    }

    fn names(roles: Vec<&Role>) -> Vec<&str> {
        roles.into_iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn mentionable_roles_unless_we_may_mention_everyone() {
        let guild = guild();
        assert_eq!(names(roles(&guild, "m", false, 0)), ["Mod", "Moderators", "Game Masters"]);
        assert_eq!(names(roles(&guild, "m", true, 0)), ["Members", "Mod", "Moderators", "Game Masters"]);
        assert!(names(roles(&guild, "every", true, 0)).is_empty(), "never @everyone's own");
    }

    #[test]
    fn best_match_first_then_by_name() {
        let guild = guild();
        // Mod equals it, Moderators starts with it.
        assert_eq!(names(roles(&guild, "Mod", false, 0)), ["Mod", "Moderators"]);
        // A word inside, then the letters in order.
        assert_eq!(names(roles(&guild, "mast", false, 0)), ["Game Masters"]);
        assert_eq!(names(roles(&guild, "gm", false, 0)), ["Game Masters"], "by its initials");
        assert_eq!(names(roles(&guild, "mdr", false, 0)), ["Moderators"]);
    }

    #[test]
    fn roles_fill_what_the_people_leave() {
        let guild = guild();
        assert_eq!(names(roles(&guild, "m", false, 8)), ["Mod", "Moderators"]);
        assert!(roles(&guild, "m", false, LIMIT).is_empty());
    }

    #[test]
    fn everyone_and_here_where_we_may_mention_everyone() {
        assert_eq!(globals("", true, false, 0), [Global::Everyone, Global::Here]);
        assert_eq!(globals("h", true, false, 0), [Global::Here]);
        assert_eq!(globals("vy", true, false, 0), [Global::Everyone], "the letters in order");
        assert_eq!(globals("ee", true, false, 0), [Global::Everyone, Global::Here]);
        assert!(globals("", false, false, 0).is_empty());
        assert!(globals("", true, true, 0).is_empty(), "not in a DM");
        assert_eq!(globals("", true, false, LIMIT - 1), [Global::Everyone], "while there's room");
    }

    #[test]
    fn people_starting_with_it_then_its_letters_in_order() {
        let person = |id: u64, username: &'static str, global_name: Option<&'static str>, nick| Person {
            id: Snowflake(id),
            username,
            global_name,
            nick,
        };
        let people = [
            person(1, "zed", Some("Ånna"), None),
            person(2, "bob", None, Some("anvil")),
            person(3, "banana", None, None),
            person(4, "carl", None, None),
        ];
        let ids = |found: Vec<Snowflake>| found.into_iter().map(|s| s.0).collect::<Vec<_>>();
        assert_eq!(ids(rank_people("an", &people, LIMIT)), [1, 2, 3], "by global name, nickname, then fuzzily");
        assert_eq!(ids(rank_people("", &people, 2)), [1, 2], "nothing typed: in order, up to the limit");
        assert!(rank_people("x", &people, LIMIT).is_empty());
    }

    #[test]
    fn match_sorter_ranks() {
        assert_eq!(rank("Mod", "Mod"), Some(7.0));
        assert_eq!(rank("Mod", "mod"), Some(6.0));
        assert_eq!(rank("Moderators", "mod"), Some(5.0));
        assert_eq!(rank("Game Masters", "mas"), Some(4.0));
        assert_eq!(rank("Moderators", "rat"), Some(3.0));
        assert_eq!(rank("Moderators", "z"), None);
        assert_eq!(rank("Game Masters", "gm"), Some(2.0));
        assert_eq!(rank("Moderators", "mdr"), Some(1.0 + 1.0 / 4.0));
        assert_eq!(rank("Café", "cafe"), Some(6.0), "accents dropped");
    }
}
