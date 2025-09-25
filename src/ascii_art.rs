use std::time::{SystemTime, UNIX_EPOCH};

pub const LOGO: &str = r#"
╔═══════════════════════════════════════════════╗
║   ___                              ___        ║
║  / __\___  _ __ ___  _ __ ___  _   / __\__ _  ║
║ / /  / _ \| '_ ` _ \| '_ ` _ \| | | / /  / _` |║
║/ /__| (_) | | | | | | | | | | | |_| / /__| (_| |║
║\____/\___/|_| |_| |_|_| |_| |_|\__,_\____/\__,_|║
║                                                ║
║         Secure P2P Messenger v0.1.0            ║
║              CCP-1 Protocol                    ║
╚═══════════════════════════════════════════════╝
"#;

pub const CAT_HAPPY: &str = r#"
   ∧___∧
  ( ≧ω≦ )
  (つ   ⊂)
   と_)_)
"#;

pub const CAT_TYPING: &str = r#"
   ∧___∧
  ( ･ω･ )
＿|⊃／(＿＿
/  └-(____/
"#;

pub const CAT_SLEEPING: &str = r#"
   ∧___∧
  ( ˘ω˘ )
  (つ   ⊂)
   Zzz...
"#;

pub const CAT_ERROR: &str = r#"
   ∧___∧
  ( ；∀；)
  (つ   ⊂)
   ERROR!
"#;

pub const KAWAII_STICKERS: &[(&str, &str)] = &[
    ("happy", "(◕‿◕)✨"),
    ("love", "♡(◡‿◡✿)"),
    ("excited", "＼(＾▽＾)／"),
    ("wink", "(◕‿-)"),
    ("peace", "✌(◕‿◕)✌"),
    ("star", "★~(◡﹏◕✿)"),
    ("music", "♪(´▽｀)"),
    ("flower", "✿(◠‿◠)✿"),
    ("bear", "ʕ•ᴥ•ʔ"),
    ("bunny", "(/_/)"),
    ("neko", "=^.^="),
    ("sparkle", "✧･ﾟ: *✧･ﾟ:*"),
];

pub fn random_kawaii() -> &'static str {
    let idx = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as usize;
    KAWAII_STICKERS[idx % KAWAII_STICKERS.len()].1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cat_ascii_art_not_empty() {
        assert!(!CAT_HAPPY.trim().is_empty());
        assert!(!CAT_TYPING.trim().is_empty());
        assert!(!CAT_SLEEPING.trim().is_empty());
        assert!(!CAT_ERROR.trim().is_empty());
    }

    #[test]
    fn random_kawaii_returns_known_value() {
        let value = random_kawaii();
        assert!(KAWAII_STICKERS.iter().any(|(_, sticker)| sticker == &value));
    }
}
