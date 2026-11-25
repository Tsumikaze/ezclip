//! Matching the running game to one of the user's existing clip folders
//! (pure logic, no Windows calls, so it is unit-testable anywhere).

/// Lowercase letters and digits only: "Counter-Strike 2" -> "counterstrike2".
pub fn normalize(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

/// Pick the folder that best matches any of the candidate names (window
/// title, product name, install folder, exe name...). Exact matches win; then
/// one name containing the other (at least 4 characters), longest first.
pub fn best_match(folders: &[String], candidates: &[String]) -> Option<usize> {
    let cands: Vec<String> = candidates.iter().map(|c| normalize(c)).filter(|c| c.len() >= 2).collect();
    let mut best: Option<(usize, (u8, usize))> = None;
    for (i, f) in folders.iter().enumerate() {
        let nf = normalize(f);
        if nf.len() < 2 {
            continue;
        }
        for nc in &cands {
            let score = if *nc == nf {
                (2, nf.len())
            } else {
                let (short, long) = if nf.len() <= nc.len() { (&nf, nc) } else { (nc, &nf) };
                if short.len() >= 4 && long.contains(short.as_str()) {
                    (1, short.len())
                } else {
                    continue;
                }
            };
            if best.map_or(true, |(_, b)| score > b) {
                best = Some((i, score));
            }
        }
    }
    best.map(|(i, _)| i)
}

/// Make a string safe as a Windows file/folder name.
pub fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { ' ' } else { c })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches(|c: char| c == '.' || c == ' ');
    trimmed.chars().take(60).collect::<String>().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn matches_typical_games() {
        let folders = f(&["Valorant", "Counter-Strike 2", "Fortnite", "Minecraft", "Minecraft Dungeons", "Rocket League", "CS2"]);
        let m = |c: &[&str]| best_match(&folders, &f(c)).map(|i| folders[i].as_str());
        assert_eq!(m(&["VALORANT  ", "VALORANT-Win64-Shipping"]), Some("Valorant"));
        assert_eq!(m(&["Counter-Strike 2", "cs2"]), Some("Counter-Strike 2"));
        assert_eq!(m(&["FortniteClient-Win64-Shipping"]), Some("Fortnite"));
        assert_eq!(m(&["Minecraft Dungeons"]), Some("Minecraft Dungeons"));
        assert_eq!(m(&["Minecraft 1.21.1 - Singleplayer", "javaw"]), Some("Minecraft"));
        assert_eq!(m(&["RocketLeague"]), Some("Rocket League"));
        assert_eq!(m(&["Google Chrome", "chrome"]), None);
        assert_eq!(m(&[]), None);
    }

    #[test]
    fn sanitizes() {
        assert_eq!(sanitize("  Foo: Bar?  "), "Foo Bar");
        assert_eq!(sanitize("a/b\\c"), "a b c");
        assert_eq!(sanitize("..."), "");
    }
}
