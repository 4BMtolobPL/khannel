//! 채널 유저 목록. 닉은 대소문자를 구분하지 않고(`nick_eq`와 같은 기준) 권한 접두사를 함께 둔다.

use std::collections::HashMap;

use khannel_core::client::event::{MemberModeChange, NamesEntry};
use serde::Serialize;

/// 005 PREFIX를 받기 전의 권한 접두사 기호 (코어 기본값과 같음, 높은 권한부터)
pub const DEFAULT_PREFIX_SYMBOLS: &str = "~&@%+";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelUser {
    pub nick: String,
    /// 가진 권한 접두사 기호 — 높은 권한 먼저 (`@+`). 첫 글자가 표시할 최고 권한이다
    pub prefixes: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ChannelUsers {
    /// 키는 소문자 닉
    by_key: HashMap<String, ChannelUser>,
}

fn key(nick: &str) -> String {
    nick.to_ascii_lowercase()
}

/// 접두사 기호의 권한 순위 (작을수록 높음). 모르는 기호는 맨 뒤
fn rank(symbol: char, symbols: &str) -> usize {
    symbols
        .chars()
        .position(|s| s == symbol)
        .unwrap_or(usize::MAX)
}

impl ChannelUsers {
    /// NAMES 응답으로 목록을 통째로 교체한다
    pub fn replace(&mut self, entries: Vec<NamesEntry>) {
        self.by_key = entries
            .into_iter()
            .map(|entry| {
                (
                    key(&entry.nick),
                    ChannelUser {
                        nick: entry.nick,
                        prefixes: entry.prefixes,
                    },
                )
            })
            .collect();
    }

    /// JOIN — 이미 있으면 그대로 둔다
    pub fn insert(&mut self, nick: &str) {
        self.by_key.entry(key(nick)).or_insert_with(|| ChannelUser {
            nick: nick.to_string(),
            prefixes: String::new(),
        });
    }

    pub fn remove(&mut self, nick: &str) -> bool {
        self.by_key.remove(&key(nick)).is_some()
    }

    /// NICK — 권한 접두사는 유지한다
    pub fn rename(&mut self, old: &str, new: &str) -> bool {
        let Some(mut user) = self.by_key.remove(&key(old)) else {
            return false;
        };
        user.nick = new.to_string();
        self.by_key.insert(key(new), user);
        true
    }

    pub fn contains(&self, nick: &str) -> bool {
        self.by_key.contains_key(&key(nick))
    }

    /// MODE의 권한 변경을 반영한다. 접두사는 `symbols`(높은 권한부터) 순서로 유지한다
    pub fn apply_mode(&mut self, change: &MemberModeChange, symbols: &str) {
        let Some(user) = self.by_key.get_mut(&key(&change.nick)) else {
            return;
        };
        let mut prefixes: Vec<char> = user
            .prefixes
            .chars()
            .filter(|&c| c != change.symbol)
            .collect();
        if change.added {
            prefixes.push(change.symbol);
        }
        prefixes.sort_by_key(|&c| rank(c, symbols));
        user.prefixes = prefixes.into_iter().collect();
    }

    pub fn clear(&mut self) {
        self.by_key.clear();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// 표시 순서: 최고 권한 순 → 닉(대소문자 무시) → 원래 닉
    pub fn sorted(&self, symbols: &str) -> Vec<ChannelUser> {
        let mut users: Vec<ChannelUser> = self.by_key.values().cloned().collect();
        users.sort_by_cached_key(|user| {
            let top = user
                .prefixes
                .chars()
                .map(|c| rank(c, symbols))
                .min()
                .unwrap_or(usize::MAX);
            (top, key(&user.nick), user.nick.clone())
        });
        users
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(prefixes: &str, nick: &str) -> NamesEntry {
        NamesEntry {
            nick: nick.to_string(),
            prefixes: prefixes.to_string(),
        }
    }

    fn display(users: &ChannelUsers) -> Vec<String> {
        users
            .sorted(DEFAULT_PREFIX_SYMBOLS)
            .into_iter()
            .map(|u| format!("{}{}", u.prefixes, u.nick))
            .collect()
    }

    #[test]
    fn sorted_by_rank_then_nick() {
        let mut users = ChannelUsers::default();
        users.replace(vec![
            entry("", "zed"),
            entry("+", "bob"),
            entry("", "Alice"),
            entry("@", "op"),
            entry("@+", "both"),
            entry("~", "owner"),
        ]);
        assert_eq!(
            display(&users),
            ["~owner", "@+both", "@op", "+bob", "Alice", "zed"]
        );
    }

    #[test]
    fn nick_lookup_ignores_case() {
        let mut users = ChannelUsers::default();
        users.replace(vec![entry("@", "Kim")]);
        assert!(users.contains("kim"));

        // 접두사는 닉 변경 후에도 유지된다
        assert!(users.rename("KIM", "Lee"));
        assert_eq!(display(&users), ["@Lee"]);

        // 이미 있는 닉의 JOIN은 접두사를 지우지 않는다
        users.insert("lee");
        assert_eq!(display(&users), ["@Lee"]);

        assert!(users.remove("LEE"));
        assert!(users.is_empty());
        assert!(!users.rename("nobody", "x"));
    }

    #[test]
    fn apply_mode_keeps_prefixes_in_rank_order() {
        let mut users = ChannelUsers::default();
        users.replace(vec![entry("+", "bob")]);
        let change = |symbol, added| MemberModeChange {
            nick: "Bob".to_string(),
            symbol,
            added,
        };

        users.apply_mode(&change('@', true), DEFAULT_PREFIX_SYMBOLS);
        assert_eq!(display(&users), ["@+bob"]);
        // 같은 권한을 다시 줘도 중복되지 않는다
        users.apply_mode(&change('@', true), DEFAULT_PREFIX_SYMBOLS);
        assert_eq!(display(&users), ["@+bob"]);
        users.apply_mode(&change('@', false), DEFAULT_PREFIX_SYMBOLS);
        assert_eq!(display(&users), ["+bob"]);
        users.apply_mode(&change('+', false), DEFAULT_PREFIX_SYMBOLS);
        assert_eq!(display(&users), ["bob"]);

        // 목록에 없는 닉은 무시
        users.apply_mode(
            &MemberModeChange {
                nick: "ghost".to_string(),
                symbol: '@',
                added: true,
            },
            DEFAULT_PREFIX_SYMBOLS,
        );
        assert_eq!(users.len(), 1);
    }
}
